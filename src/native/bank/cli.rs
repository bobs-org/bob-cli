//! `bob bank` list, report, ensure, ack, and check.

use std::{collections::HashMap, ffi::OsString, fs, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{
    ack, check, dates, ensure, import_run, list, output, report, scan, status,
};
use crate::native::env as bob_env;

const COMMAND_NAME: &str = "bob bank";

fn print_clap_error(error: clap::Error) -> i32 {
    let code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    code
}

fn run_with(
    mut command: ClapCommand,
    args: Vec<OsString>,
    execute: fn(&ArgMatches) -> i32,
) -> i32 {
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    execute(&matches)
}

pub(crate) fn run_check(args: Vec<OsString>) -> i32 {
    run_with(build_cli_check(), args, execute_check)
}

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    run_with(build_cli_list(), args, execute_list)
}

pub(crate) fn run_report(args: Vec<OsString>) -> i32 {
    run_with(build_cli_report(), args, execute_report)
}

pub(crate) fn run_ensure(args: Vec<OsString>) -> i32 {
    run_with(build_cli_ensure(), args, execute_ensure)
}

pub(crate) fn run_ack(args: Vec<OsString>) -> i32 {
    run_with(build_cli_ack(), args, execute_ack)
}

pub(crate) fn run_import(args: Vec<OsString>) -> i32 {
    run_with(build_cli_import(), args, execute_import)
}

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob bank check")
        .about("Check bank notes against the contract")
        .long_about(
            "Findings grouped by note, then day, plus an error/warning summary. Exits 1 on any error, 0 otherwise. This command is read-only. With no bank.md and no bank/ folder, prints a teaching empty state and exits 0.",
        )
        .after_help(
            "Examples:\n  bob bank check\n  bob bank check -f json\n\nSee docs/bank.md for the bank contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_list() -> ClapCommand {
    ClapCommand::new("bob bank list")
        .about("List waiting bank rows (default)")
        .long_about(
            "The waiting queue across every month, grouped by day, oldest first. Bare `bob bank` runs this command. `-a` shows all rows; `-m` limits the output to one month. This command is read-only. With no bank.md and no bank/ folder, prints a teaching empty state and exits 0.",
        )
        .after_help(
            "Examples:\n  bob bank\n  bob bank list\n  bob bank list -a\n  bob bank list -m 2026-11 -f json\n\nSee docs/bank.md for the bank contract.",
        )
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(month_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_report() -> ClapCommand {
    ClapCommand::new("bob bank report")
        .about("Report bank totals for a day, week, month, or year")
        .long_about(
            "Every total is computed from rows at read time; rows on cash-kind accounts and rows with [cat:: transfer] are kept apart. Takes at most one period and defaults to the current month. `-D` yields one day. This command is read-only. With no bank.md and no bank/ folder, prints a teaching empty state and exits 0.",
        )
        .after_help(
            "Examples:\n  bob bank report\n  bob bank report -m 2026-11\n  bob bank report -w 2026-W46 -f json\n  bob bank report -D 2026-11-14\n\nSee docs/bank.md for the totals contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(day_arg())
        .arg(format_arg())
        .arg(month_arg())
        .arg(week_arg())
        .arg(year_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_ensure() -> ClapCommand {
    ClapCommand::new("bob bank ensure")
        .about("Ensure the month note and day heading exist")
        .long_about(
            "Creates the folder, month note, and day heading on demand; idempotent. Prints the path and the line where the next row goes (after the day's last row, or the blank line under the heading). A guarded write with no commit. Exits 2 on an invalid day or configuration.",
        )
        .after_help(
            "Examples:\n  bob bank ensure\n  bob bank ensure -D 2026-11-14\n  bob bank ensure -D today -o\n\nSee docs/bank.md for the bank contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(day_arg_default_today())
        .arg(format_arg())
        .arg(open_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_ack() -> ClapCommand {
    ClapCommand::new("bob bank ack")
        .about("Acknowledge waiting bank rows")
        .long_about(
            "Ticks the named waiting rows, every waiting row of -D DAY, or both. A guarded, capture-style write with no commit. A row that is already ticked is a no-op. Refuses unknown IDs with exit 2 and changes nothing on a refusal. `-d` previews diffs without writing.",
        )
        .after_help(
            "Examples:\n  bob bank ack 8f3a2c1d\n  bob bank ack -D 2026-11-10\n  bob bank ack -d -D today -f json\n\nSee docs/bank.md for the bank contract.",
        )
        .disable_help_flag(true)
        .arg(ids_arg())
        .arg(bob_dir_arg())
        .arg(day_arg_optional())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_import() -> ClapCommand {
    ClapCommand::new("bob bank import")
        .about("Import posted bank transactions from QFX, OFX, or Wells Fargo CSV")
        .long_about(
            "Plans the whole batch before writing. Existing rows are matched only by account, exact signed cents, and the documented posted-date window; new rows and the private bank journal commit together. `-F` deliberately restores journaled transactions whose rows were deleted. An ignored journal destination is refused before rows are installed. Refused input exits 2; runtime failures exit 1.",
        )
        .after_help(
            "Examples:\n  bob bank import ~/Downloads/*.qfx\n  bob bank import -d ~/Downloads/checking.qfx\n  bob bank import -a checking -f json ~/Downloads/export.csv\n  bob bank import -F\n\nSee docs/bank.md for import matching and recovery behavior.",
        )
        .disable_help_flag(true)
        .arg(
            Arg::new("files")
                .value_name("FILE")
                .num_args(0..)
                .value_parser(OsStringValueParser::new())
                .help("QFX/OFX or the supported Wells Fargo CSV export; repeatable"),
        )
        .arg(
            Arg::new("account")
                .long("account")
                .short('a')
                .value_name("ALIAS")
                .value_parser(OsStringValueParser::new())
                .help("Choose a configured importable account"),
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(restore_missing_arg())
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
        .default_value("human")
        .value_parser(["human", "json"])
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Show all rows, not only waiting rows")
}

fn month_arg() -> Arg {
    Arg::new("month")
        .long("month")
        .short('m')
        .value_name("MONTH")
        .value_parser(OsStringValueParser::new())
        .help("Limit to one month: YYYY-MM")
}

fn week_arg() -> Arg {
    Arg::new("week")
        .long("week")
        .short('w')
        .value_name("WEEK")
        .value_parser(OsStringValueParser::new())
        .help("Report one ISO week: YYYY-Www (Monday first)")
}

fn year_arg() -> Arg {
    Arg::new("year")
        .long("year")
        .short('y')
        .value_name("YEAR")
        .value_parser(OsStringValueParser::new())
        .help("Report one year: YYYY")
}

fn day_arg() -> Arg {
    Arg::new("day")
        .long("day")
        .short('D')
        .value_name("DAY")
        .value_parser(OsStringValueParser::new())
        .help("Report one day: today, yesterday, tomorrow, or YYYY-MM-DD")
}

fn day_arg_default_today() -> Arg {
    Arg::new("day")
        .long("day")
        .short('D')
        .value_name("DAY")
        .default_value("today")
        .value_parser(OsStringValueParser::new())
        .help("Day to ensure: today, yesterday, tomorrow, or YYYY-MM-DD")
}

fn day_arg_optional() -> Arg {
    Arg::new("day")
        .long("day")
        .short('D')
        .value_name("DAY")
        .value_parser(OsStringValueParser::new())
        .help("Acknowledge every waiting row of this day")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview diffs without writing")
}

fn restore_missing_arg() -> Arg {
    Arg::new("restore-missing")
        .long("force")
        .short('F')
        .action(ArgAction::SetTrue)
        .help("Restore journaled transactions whose rows are missing")
}

fn open_arg() -> Arg {
    Arg::new("open")
        .long("open")
        .short('o')
        .action(ArgAction::SetTrue)
        .help("Open the note in Obsidian after ensuring")
}

fn ids_arg() -> Arg {
    Arg::new("ids")
        .value_name("ID")
        .num_args(0..)
        .value_parser(OsStringValueParser::new())
        .help("Bank row IDs to acknowledge (^bank-hex, bank-hex, or hex)")
}

fn vault_from(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn format_is_json(matches: &ArgMatches) -> bool {
    matches.get_one::<String>("format").map(String::as_str) == Some("json")
}

fn opt_string(matches: &ArgMatches, key: &str) -> Option<String> {
    matches
        .get_one::<OsString>(key)
        .map(|value| value.to_string_lossy().into_owned())
}

fn print_io_errors(errors: &[String]) {
    for error in errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
}

fn execute_import(matches: &ArgMatches) -> i32 {
    let vault = vault_from(matches);
    let json = format_is_json(matches);
    let dry_run = matches.get_flag("dry-run");
    let force_restore = matches.get_flag("restore-missing");
    let now = bob_env::current_datetime();
    let today = now.date();
    let now_stamp = import_run::now_stamp(now);
    let paths = matches
        .get_many::<OsString>("files")
        .map(|files| files.map(PathBuf::from).collect::<Vec<_>>())
        .unwrap_or_default();
    let requested_account = opt_string(matches, "account");
    if paths.is_empty() && !force_restore {
        let report = import_run::ImportReport::failed(
            dry_run,
            today,
            import_run::missing_files_failure(),
        );
        if json {
            println!("{}", report.to_json());
        } else {
            import_run::print_human(&report);
        }
        return report.exit_code();
    }
    let prepared = match import_run::prepare_sources(&paths) {
        Ok(prepared) => prepared,
        Err(error) => {
            let report = import_run::ImportReport::failed(
                dry_run,
                today,
                import_run::source_error_failure(error),
            );
            if json {
                println!("{}", report.to_json());
            } else {
                import_run::print_human(&report);
            }
            return report.exit_code();
        }
    };
    let report = if dry_run {
        import_run::dry_run_with_restore(
            &vault,
            &prepared,
            requested_account.as_deref(),
            &now_stamp,
            today,
            force_restore,
        )
    } else {
        import_run::apply_with_restore(
            &vault,
            &prepared,
            requested_account.as_deref(),
            &now_stamp,
            today,
            json,
            force_restore,
        )
    };
    let mut report = report;
    if !dry_run && report.failure.is_none() {
        import_run::refresh_accounts(&mut report, &vault, today);
    }
    if json {
        println!("{}", report.to_json());
    } else {
        import_run::print_human(&report);
    }
    report.exit_code()
}

fn execute_check(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let scan = scan::scan_bank(&bob_dir);
    print_io_errors(&scan.io_errors);
    if !scan.io_errors.is_empty() && !json {
        return 1;
    }
    let today = bob_env::current_datetime().date();
    let report = check::lint(&bob_dir, &scan, today);
    if json {
        println!("{}", report.to_json());
    } else if scan.io_errors.is_empty() {
        output::print_check(&report);
    }
    report.exit_code()
}

fn load_config(
    scan: &scan::BankScan,
) -> Result<(super::config::BankConfig, Vec<super::config::Category>), String> {
    let Some(hub) = &scan.hub else {
        return Err("bank.md is missing".to_owned());
    };
    match super::config::parse_bank_config(&hub.contents) {
        super::config::ConfigParse::Ok(config) => {
            let categories = config.categories.clone();
            Ok((config, categories))
        }
        super::config::ConfigParse::MissingMapping => {
            Err("bank.md has no bank: mapping".to_owned())
        }
        super::config::ConfigParse::Invalid { diagnostics, .. } => {
            let first = diagnostics
                .first()
                .map(|diagnostic| {
                    format!("{}: {}", diagnostic.key_path, diagnostic.message)
                })
                .unwrap_or_else(|| "invalid bank configuration".to_owned());
            Err(first)
        }
    }
}

fn execute_list(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let today = bob_env::current_datetime().date();
    let scan = scan::scan_bank(&bob_dir);
    print_io_errors(&scan.io_errors);
    if !scan.io_errors.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "error": scan.io_errors.join("; "),
                })
            );
        }
        return 1;
    }
    if scan::is_teaching_empty(&scan) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "teaching_empty": true,
                })
            );
        } else {
            output::print_teaching_empty(today);
        }
        return 0;
    }
    let month_filter = match opt_string(matches, "month") {
        None => None,
        Some(raw) => match list::parse_month_filter(Some(&raw)) {
            Ok(filter) => filter,
            Err(message) => {
                report_option_error("bob bank list", json, today, &message);
                return 2;
            }
        },
    };
    let config = match load_config(&scan) {
        Ok((config, _)) => Some(config),
        Err(message) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob bank list: {message}");
            }
            return 1;
        }
    };
    let categories = config
        .as_ref()
        .map(|config| config.categories.clone())
        .unwrap_or_default();
    let journal = super::journal::load(&bob_dir).unwrap_or_default();
    let rows =
        super::model::collect_rows(&scan, config.as_ref(), Some(&journal));
    let accounts = config
        .as_ref()
        .map(|config| {
            status::accounts_with_evidence(config, &journal, &rows, today).0
        })
        .unwrap_or_default();
    let default_alias = config
        .as_ref()
        .and_then(|config| config.default_account())
        .map(|account| account.alias.clone())
        .unwrap_or_else(|| "checking".to_owned());
    let heading_lines = list::collect_heading_lines(&scan);
    let report = list::build(
        rows,
        today,
        list::ListFilter {
            all: matches.get_flag("all"),
            month: month_filter,
        },
        categories,
        accounts,
        heading_lines,
    );
    if json {
        println!("{}", list::to_json(&report, &default_alias));
    } else {
        output::print_list(&report, today);
    }
    0
}

fn execute_report(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let today = bob_env::current_datetime().date();
    let scan = scan::scan_bank(&bob_dir);
    print_io_errors(&scan.io_errors);
    if !scan.io_errors.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "error": scan.io_errors.join("; "),
                })
            );
        }
        return 1;
    }
    if scan::is_teaching_empty(&scan) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "teaching_empty": true,
                })
            );
        } else {
            output::print_teaching_empty(today);
        }
        return 0;
    }
    let day = opt_string(matches, "day");
    let month = opt_string(matches, "month");
    let week = opt_string(matches, "week");
    let year = opt_string(matches, "year");
    let period = match report::resolve_period(
        day.as_deref(),
        month.as_deref(),
        week.as_deref(),
        year.as_deref(),
        today,
    ) {
        Ok(period) => period,
        Err(message) => {
            report_option_error("bob bank report", json, today, &message);
            return 2;
        }
    };
    let config = match load_config(&scan) {
        Ok((config, _)) => Some(config),
        Err(message) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob bank report: {message}");
            }
            return 1;
        }
    };
    let categories = config
        .as_ref()
        .map(|config| config.categories.clone())
        .unwrap_or_default();
    let default_alias = config
        .as_ref()
        .and_then(|config| config.default_account())
        .map(|account| account.alias.clone())
        .unwrap_or_else(|| "checking".to_owned());
    let journal = super::journal::load(&bob_dir).unwrap_or_default();
    let rows =
        super::model::collect_rows(&scan, config.as_ref(), Some(&journal));
    let accounts = config
        .as_ref()
        .map(|config| {
            status::accounts_with_evidence(config, &journal, &rows, today).0
        })
        .unwrap_or_default();
    let heading_lines = list::collect_heading_lines(&scan);
    let built = report::build(
        rows,
        today,
        period,
        config.as_ref(),
        categories,
        accounts,
        default_alias,
        &heading_lines,
    );
    if json {
        println!("{}", report::to_json(&built));
    } else {
        output::print_report(&built);
    }
    0
}

fn execute_ensure(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let today = bob_env::current_datetime().date();
    let scan = scan::scan_bank(&bob_dir);
    print_io_errors(&scan.io_errors);
    if !scan.io_errors.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "error": scan.io_errors.join("; "),
                })
            );
        }
        return 1;
    }
    if scan::is_teaching_empty(&scan) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "teaching_empty": true,
                })
            );
        } else {
            output::print_teaching_empty(today);
        }
        return 0;
    }
    let raw_day = opt_string(matches, "day");
    let day = match ensure::resolve_ensure_day(raw_day.as_deref(), today) {
        Ok(day) => day,
        Err(message) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob bank ensure: {message}");
            }
            return 2;
        }
    };
    let (config, _) = match load_config(&scan) {
        Ok(loaded) => loaded,
        Err(message) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob bank ensure: {message}");
            }
            return 2;
        }
    };
    let hub_contents = scan
        .hub
        .as_ref()
        .map(|hub| hub.contents.as_str())
        .unwrap_or("");
    let plan = match ensure::plan(&bob_dir, day, hub_contents, &config) {
        Ok(plan) => plan,
        Err(message) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob bank ensure: {message}");
            }
            return 1;
        }
    };
    if let Some(updated) = &plan.updated_contents {
        let staged = crate::native::capture::StagedTextFile {
            target: plan.target.clone(),
            target_existed: plan.existed,
            original_target: plan.original.clone(),
            updated_target: updated.clone(),
        };
        if let Err(error) = crate::native::capture::write_staged_files_guarded(
            std::slice::from_ref(&staged),
            || Ok(()),
        ) {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": error.message,
                    })
                );
            } else {
                eprintln!("bob bank ensure: {}", error.message);
            }
            return 1;
        }
    }
    if matches.get_flag("open") {
        let relative = std::path::Path::new(&plan.path);
        if let Err(message) = ensure::open_in_obsidian(&bob_dir, relative) {
            eprintln!("bob bank ensure: warning: {message}");
        }
    }
    if json {
        let categories = config.categories.clone();
        let journal = super::journal::load(&bob_dir).unwrap_or_default();
        let rows =
            super::model::collect_rows(&scan, Some(&config), Some(&journal));
        let accounts =
            status::accounts_with_evidence(&config, &journal, &rows, today).0;
        println!("{}", ensure::to_json(&plan, today, &categories, &accounts));
    } else {
        output::print_ensure(
            &plan.path,
            plan.line,
            plan.day,
            plan.created_note,
            plan.created_heading,
        );
    }
    0
}

fn execute_ack(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let dry_run = matches.get_flag("dry-run");
    let today = bob_env::current_datetime().date();
    let scan = scan::scan_bank(&bob_dir);
    print_io_errors(&scan.io_errors);
    if !scan.io_errors.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "error": scan.io_errors.join("; "),
                })
            );
        }
        return 1;
    }
    if scan::is_teaching_empty(&scan) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "teaching_empty": true,
                })
            );
        } else {
            output::print_teaching_empty(today);
        }
        return 0;
    }
    let ids: Vec<String> = matches
        .get_many::<OsString>("ids")
        .map(|values| {
            values
                .map(|value| value.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let day = match opt_string(matches, "day") {
        None => None,
        Some(raw) => match dates::resolve_day(&raw, today) {
            Ok(day) => Some(day),
            Err(message) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "ok": false,
                            "schema_version": 1,
                            "today": today.format("%Y-%m-%d").to_string(),
                            "error": message,
                        })
                    );
                } else {
                    eprintln!("bob bank ack: {message}");
                }
                return 2;
            }
        },
    };
    if ids.is_empty() && day.is_none() {
        let message = "pass at least one ID or -D/--day".to_owned();
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "schema_version": 1,
                    "today": today.format("%Y-%m-%d").to_string(),
                    "error": message,
                })
            );
        } else {
            eprintln!("bob bank ack: {message}");
        }
        return 2;
    }
    let config = match load_config(&scan) {
        Ok((config, _)) => Some(config),
        Err(message) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob bank ack: {message}");
            }
            return 1;
        }
    };
    let journal = super::journal::load(&bob_dir).ok();
    let rows =
        super::model::collect_rows(&scan, config.as_ref(), journal.as_ref());
    let mut file_contents: HashMap<String, String> = HashMap::new();
    for month in &scan.months {
        file_contents.insert(month.path.clone(), month.contents.clone());
    }
    let selection = ack::AckSelection { ids, day };
    let plan = ack::plan_ack(&rows, &selection, &file_contents);
    let categories = config
        .as_ref()
        .map(|config| config.categories.clone())
        .unwrap_or_default();
    let accounts = config.as_ref().map(status::accounts).unwrap_or_default();
    if !plan.unknown.is_empty() {
        if json {
            let diffs = HashMap::new();
            println!(
                "{}",
                ack::to_json(
                    &plan,
                    today,
                    dry_run,
                    false,
                    &diffs,
                    &categories,
                    &accounts
                )
            );
        } else {
            output::print_ack(
                &plan.acknowledged,
                &plan.already,
                &plan.unknown,
                dry_run,
            );
        }
        return 2;
    }
    if plan.acknowledged.is_empty() {
        if json {
            let diffs = HashMap::new();
            println!(
                "{}",
                ack::to_json(
                    &plan,
                    today,
                    dry_run,
                    false,
                    &diffs,
                    &categories,
                    &accounts
                )
            );
        } else {
            output::print_ack(&plan.acknowledged, &plan.already, &[], dry_run);
        }
        return 0;
    }
    let mut diffs: HashMap<String, String> = HashMap::new();
    if dry_run {
        for (path, updated) in &plan.edits {
            let before = plan.originals.get(path).cloned().unwrap_or_default();
            diffs.insert(
                path.clone(),
                similar::TextDiff::from_lines(&before, updated)
                    .unified_diff()
                    .context_radius(3)
                    .header(&format!("a/{path}"), &format!("b/{path}"))
                    .to_string(),
            );
        }
    } else {
        let files = ack::staged_files(&bob_dir, &plan);
        if let Err(error) = crate::native::capture::write_staged_files_guarded(
            &files,
            || Ok(()),
        ) {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "schema_version": 1,
                        "today": today.format("%Y-%m-%d").to_string(),
                        "error": error.message,
                    })
                );
            } else {
                eprintln!("bob bank ack: {}", error.message);
            }
            return 1;
        }
    }
    // Re-read to guard against two writers ticking the same row: the
    // guarded writer already refused a changed preimage above.
    let _ = fs::read_to_string(bob_dir.join("bank.md"));
    if json {
        println!(
            "{}",
            ack::to_json(
                &plan,
                today,
                dry_run,
                true,
                &diffs,
                &categories,
                &accounts
            )
        );
    } else {
        output::print_ack(&plan.acknowledged, &plan.already, &[], dry_run);
    }
    0
}

fn report_option_error(
    command: &str,
    json: bool,
    today: chrono::NaiveDate,
    message: &str,
) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": false,
                "schema_version": 1,
                "today": today.format("%Y-%m-%d").to_string(),
                "error": message,
            })
        );
    } else {
        eprintln!("{command}: {message}");
    }
}
