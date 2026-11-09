use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs, io,
    path::{Component, Path, PathBuf},
    process::{self, Command, Output, Stdio},
};

use clap::{Arg, ArgAction, Command as ClapCommand};

use super::{
    env as bob_env,
    ob::{self, ChildEnv},
};

mod archive;
mod command;
mod git;
mod link_repair;
mod plan;
mod report;
mod snapshot;
#[cfg(test)]
mod tests;
mod transform;

use archive::*;
use link_repair::*;
use plan::*;
use report::OutputFormat;
use transform::*;

pub(crate) use archive::atomic_write;
pub(crate) use command::run_collection;
pub(crate) use link_repair::{
    archive_parent_target, source_relative_path_from_archive,
};
pub(crate) use transform::{
    block_ids_in_markdown, is_block_id_byte, split_line_ending,
    trailing_block_id_in_line,
};

pub(crate) use transform::link_repair_markdown_files;

pub(crate) use archive::next_available_block_id;
#[cfg(test)]
pub(crate) use link_repair::repair_links_in_note;
pub(crate) use link_repair::{
    is_simple_markdown_destination, repair_dependency_metadata,
    repair_links_in_note_with, repair_markdown_destination,
    repair_wiki_link_inner, split_block_fragment, LinkRepairResult,
    MovedBlockTarget, MovedBlockTargets,
};
pub(crate) use transform::{
    block_id_occurrences_in_text, has_task_tag, leading_indent_len,
    task_block_end, vault_relative_link_target,
};

/// Return changed note bytes from the production archive planner for a
/// temporary fixture. Tests call the planner directly, without invoking the
/// disk-facing command or applying its write plan.
#[cfg(test)]
pub(crate) fn collection_postimages_for_tests(
    vault: &Path,
    threshold: usize,
) -> io::Result<BTreeMap<PathBuf, String>> {
    let plan = plan::build_collection_plan(vault, threshold)?;
    let mut postimages = BTreeMap::new();
    for file in plan.files {
        if file.writes_source() {
            postimages.insert(file.relative_source_path, file.source_contents);
        }
        if let Some(contents) = file.archive_contents {
            postimages.insert(file.relative_archive_path, contents);
        }
    }
    for repair in plan.link_repairs {
        postimages.insert(repair.relative_path, repair.contents);
    }
    Ok(postimages)
}

pub(super) const COMMAND_NAME: &str = "bob task archive";
pub(crate) const DEFAULT_THRESHOLD: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Args {
    threshold: usize,
    dry_run: bool,
    format: OutputFormat,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            dry_run: false,
            format: OutputFormat::Human,
        }
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    match parse_args(args) {
        ParseResult::Run(args) => command::run_standalone(args),
        ParseResult::Help => {
            print_help();
            0
        }
        ParseResult::Error(message) => {
            eprintln!("{COMMAND_NAME}: {message}");
            eprintln!("Try '{COMMAND_NAME} --help' for more information.");
            2
        }
    }
}

fn merged_output(output: &Output) -> String {
    let mut merged = String::new();
    merged.push_str(&String::from_utf8_lossy(&output.stdout));
    merged.push_str(&String::from_utf8_lossy(&output.stderr));
    merged
}

fn write_stderr_output(output: &str) {
    if !output.is_empty() {
        eprint!("{output}");
    }
}

fn parse_args(args: Vec<OsString>) -> ParseResult {
    let mut parsed = Args::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        let text = bob_env::os_to_string(&arg);
        match text.as_str() {
            "-h" | "--help" => return ParseResult::Help,
            "-n" | "--dry-run" => parsed.dry_run = true,
            "-t" | "--threshold" => {
                let option = text.as_str();
                let Some(value) = args.next() else {
                    return ParseResult::Error(format!(
                        "option {option} requires a value"
                    ));
                };
                parsed.threshold = match parse_threshold(&value) {
                    Ok(threshold) => threshold,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            "-f" | "--format" => {
                let option = text.as_str();
                let Some(value) = args.next() else {
                    return ParseResult::Error(format!(
                        "option {option} requires a value"
                    ));
                };
                parsed.format =
                    match parse_format(&bob_env::os_to_string(&value)) {
                        Ok(format) => format,
                        Err(message) => return ParseResult::Error(message),
                    };
            }
            "--" => {
                if let Some(extra) = args.next() {
                    return ParseResult::Error(format!(
                        "unexpected positional argument: {}",
                        bob_env::os_to_string(&extra)
                    ));
                }
            }
            _ if let Some(value) = text.strip_prefix("--threshold=") => {
                parsed.threshold = match parse_threshold_text(value) {
                    Ok(threshold) => threshold,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            _ if let Some(value) = text.strip_prefix("--format=") => {
                parsed.format = match parse_format(value) {
                    Ok(format) => format,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            _ if let Some(value) = text.strip_prefix("-t=") => {
                parsed.threshold = match parse_threshold_text(value) {
                    Ok(threshold) => threshold,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            _ if let Some(value) = text.strip_prefix("-f=") => {
                parsed.format = match parse_format(value) {
                    Ok(format) => format,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            _ if text.starts_with("-t") => {
                let value = &text[2..];
                parsed.threshold = match parse_threshold_text(value) {
                    Ok(threshold) => threshold,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            _ if text.starts_with("-f") => {
                let value = &text[2..];
                parsed.format = match parse_format(value) {
                    Ok(format) => format,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            _ if text.starts_with('-') => {
                return ParseResult::Error(format!(
                    "unrecognized argument: {text}"
                ));
            }
            _ => {
                return ParseResult::Error(format!(
                    "unexpected positional argument: {text}"
                ));
            }
        }
    }

    ParseResult::Run(parsed)
}

enum ParseResult {
    Run(Args),
    Help,
    Error(String),
}

fn parse_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "human" => Ok(OutputFormat::Human),
        "json" => Ok(OutputFormat::Json),
        _ => Err(format!(
            "invalid --format value: {value} (expected human or json)"
        )),
    }
}

fn parse_threshold(value: &OsString) -> Result<usize, String> {
    parse_threshold_text(&bob_env::os_to_string(value))
}

fn parse_threshold_text(value: &str) -> Result<usize, String> {
    let threshold = value
        .parse::<usize>()
        .map_err(|_| format!("invalid --threshold value: {value}"))?;
    if threshold == 0 {
        return Err("--threshold must be at least 1".to_string());
    }

    Ok(threshold)
}

pub(crate) fn completion_descriptor() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Move done and canceled tasks from area and project notes")
        .disable_help_flag(true)
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('n')
                .action(ArgAction::SetTrue)
                .help("Plan and report without writing or touching Git"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .help("Output format: human (default) or json"),
        )
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::Help)
                .help("Show this help message and exit"),
        )
        .arg(
            Arg::new("threshold")
                .long("threshold")
                .short('t')
                .value_name("N")
                .value_parser(clap::value_parser!(usize))
                .help("Minimum completed/canceled task count per source note"),
        )
}

pub(crate) fn help_text() -> String {
    format!(
        "\
usage: {COMMAND_NAME} [-n|--dry-run] [-f|--format human|json] [-t|--threshold N]

Move done and canceled Bob task blocks from area and project notes into
done/ archives, link sources, repair archive metadata, and repair Obsidian
links to moved block ids. Writes go through the guarded writer under the
vault maintenance lock (waiting up to 30 seconds). See docs/task-archive.md.

examples:
  {COMMAND_NAME} --threshold 10
  {COMMAND_NAME} --dry-run
  {COMMAND_NAME} -n -f json

options:
  -n, --dry-run    plan and report only: no lock, writes, or Git
  -f, --format human|json
                   output format (default: human); json prints one
                   schema_version 1 document
  -h, --help       show this help message and exit
  -t, --threshold N
                   minimum completed/canceled task count per source note \
(default: {DEFAULT_THRESHOLD})"
    )
}

fn print_help() {
    println!("{}", help_text());
}
