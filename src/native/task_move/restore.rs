//! `bob task restore`: bring archived tasks back to their source notes.
//!
//! A thin wrapper over the task-move engine: the request sets
//! restore mode (done/ sources allowed, empty destination resolves to the
//! archive's bound source) with closed selections accepted, and optionally
//! reopen. Planning, guarded apply, lock, sync, commit, and reporting all
//! run through the shared move pipeline with restore-flavored names.

use std::{ffi::OsString, iter};

use clap::{Arg, ArgAction, ArgGroup, Command as ClapCommand};

use super::command::{
    self, CommandKind, OutputFormat, DEFAULT_RETRY_TIMEOUT_SECS,
};
use super::{IdCollisionPolicy, MovePlacement, MoveRequest};

pub(crate) const COMMAND_NAME: &str = "bob task restore";
pub(crate) const TOOL_NAME: &str = "task-restore";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return command::print_clap_error(error),
    };

    let dry_run = matches.get_flag("dry-run");
    let offline = matches.get_flag("offline");
    let reopen = matches.get_flag("reopen");
    let format = OutputFormat::from_matches(&matches);
    let mut report = command::Report::new(
        format,
        dry_run,
        offline,
        DEFAULT_RETRY_TIMEOUT_SECS,
    );
    report.kind = CommandKind::Restore;
    let Some(source) = matches.get_one::<String>("source").cloned() else {
        eprintln!(
            "{COMMAND_NAME}: the following required arguments were not provided:\n  <SOURCE>"
        );
        return 2;
    };
    let dest = matches
        .get_one::<String>("dest")
        .cloned()
        .unwrap_or_default();
    let selectors = command::selectors_from_matches(&matches);
    if selectors.is_empty() {
        eprintln!(
            "{COMMAND_NAME}: the following required arguments were not provided:\n  (-b <ID> | -m <TEXT>)..."
        );
        return 2;
    }
    let request = MoveRequest {
        source,
        dest,
        selectors,
        allow_closed: true,
        placement: MovePlacement::Append,
        id_collision: IdCollisionPolicy::Rename,
        restore: true,
        reopen,
    };
    let exit = command::execute(&mut report, &request);
    command::print_report(&report);
    exit
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Restore archived tasks to their source notes and repair vault links")
        .long_about("Move selected closed #task blocks out of a done/ archive back into a live note, repairing block links and dependency identities throughout the vault.\n\nThe destination defaults to the archive's bound source through the shared pairing resolver, so most restores name only the archive. Pass -d/--dest to land in a different area or eligible project note instead. With -r/--reopen the restored root returns open ([ ]) with its [completion::]/[cancelled::] field removed; without it the task returns closed. Recurring occurrences, merged-task tombstones, and ^prj lines are refused.\n\nA live run holds the shared maintenance lock, syncs before and after the restore, and commits exactly the changed Markdown files. Use --dry-run to inspect the same plan without locking, syncing, probing Git, or writing. Use --offline to skip both sync cycles while keeping the lock and scoped local commit.")
        .after_help("Examples:\n  bob task restore --dry-run done/work_done -b old-task\n                                 Preview restoring one archived task home\n  bob task restore done/work_done -m 'Quarterly review'\n                                 Restore a task selected by literal line text\n  bob task restore done/work_done -b old-task -r\n                                 Restore and reopen the task\n  bob task restore -d projects/launch done/work_done -b old-task\n                                 Restore into a different note\n\nSee [Restore archived tasks](docs/task-archive.md#restore) for the bound-source rule, refusals, and reopen behavior.")
        .override_usage("bob task restore [OPTIONS] <SOURCE> (-b ID | -m TEXT)...")
        .disable_help_flag(true)
        .arg(Arg::new("source").value_name("SOURCE").required(false).index(1).help("Archive note stem or vault-relative path under done/"))
        .arg(Arg::new("block").long("block").short('b').value_name("ID").action(ArgAction::Append).help("Select a task by trailing block id without ^; repeatable"))
        .arg(Arg::new("dest").long("dest").short('d').value_name("NOTE").help("Restore into this area or project note instead of the bound source"))
        .arg(Arg::new("dry-run").long("dry-run").short('n').action(ArgAction::SetTrue).help("Plan and print only; do not lock, sync, probe Git, or write"))
        .arg(Arg::new("format").long("format").short('f').value_name("FORMAT").value_parser(["human", "json"]).default_value("human").help("Output format: human or json"))
        .arg(Arg::new("help").long("help").short('h').action(ArgAction::Help).help("Show help"))
        .arg(Arg::new("match").long("match").short('m').value_name("TEXT").action(ArgAction::Append).help("Select the task whose line contains this literal text; repeatable"))
        .arg(Arg::new("offline").long("offline").short('o').action(ArgAction::SetTrue).help("Skip both sync cycles; commit locally without pushing"))
        .arg(Arg::new("reopen").long("reopen").short('r').action(ArgAction::SetTrue).help("Restore the task open: set [ ] and remove its close field"))
        .group(
            ArgGroup::new("selector")
                .args(["block", "match"])
                .required(false)
                .multiple(true),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn clap_accepts_restore_selectors_and_dest_override() {
        let matches = build_cli()
            .try_get_matches_from(flag_args(&[
                COMMAND_NAME,
                "done/work_done",
                "-d",
                "projects/launch",
                "-b",
                "old-task",
                "-m",
                "Quarterly",
                "-r",
            ]))
            .expect("restore flags parse");
        assert_eq!(
            matches.get_one::<String>("source").map(String::as_str),
            Some("done/work_done")
        );
        assert_eq!(
            matches.get_one::<String>("dest").map(String::as_str),
            Some("projects/launch")
        );
        assert!(matches.get_flag("reopen"));
        assert_eq!(command::selectors_from_matches(&matches).len(), 2);
    }

    #[test]
    fn clap_rejects_extra_positionals() {
        build_cli()
            .try_get_matches_from(flag_args(&[
                COMMAND_NAME,
                "done/work_done",
                "projects/launch",
                "-b",
                "old-task",
            ]))
            .expect_err("a second positional is a usage error");
    }
}
