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
mod git;
mod link_repair;
mod plan;
#[cfg(test)]
mod tests;
mod transform;

use archive::*;
use git::*;
use link_repair::*;
use plan::*;
use transform::*;

pub(crate) use archive::atomic_write;
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
}

impl Default for Args {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
        }
    }
}
pub(crate) fn run(args: Vec<OsString>) -> i32 {
    match parse_args(args) {
        ParseResult::Run(args) => {
            let child_env = ob::child_env();
            run_collection(args.threshold, &child_env)
        }
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
/// Run the archive collection against the vault and commit/push the result.
///
/// This does **not** reconcile the full vault; `nightly` runs vault-sync
/// before and after it. The standalone `run()` wraps this directly.
pub(crate) fn run_collection(threshold: usize, child_env: &ChildEnv) -> i32 {
    let vault = bob_env::bob_dir();

    println!("Move done tasks");
    println!("vault: {}", vault.display());
    println!("threshold: {threshold}");

    let plan = match build_collection_plan(&vault, threshold) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!(
                "{COMMAND_NAME}: failed to scan {}: {error}",
                vault.display()
            );
            return 1;
        }
    };

    println!("scan:");
    println!("  markdown files: {}", plan.scanned_files);
    println!("  files meeting threshold: {}", plan.task_move_file_count());
    println!("  task blocks: {}", plan.total_task_count());
    print_selection_report(&plan);
    println!(
        "  source done_tasks updates: {}",
        plan.source_metadata_update_count()
    );
    println!(
        "  archive metadata repairs: {}",
        plan.archive_metadata_update_count()
    );
    println!("  moved block ids: {}", plan.moved_block_id_count());
    println!(
        "  ambiguous moved block ids: {}",
        plan.ambiguous_moved_block_id_count()
    );
    println!(
        "  moved block id renames: {}",
        plan.moved_block_id_rename_count()
    );
    println!("  Obsidian links repaired: {}", plan.link_repair_count());
    println!(
        "  dependency metadata repaired: {}",
        plan.dependency_metadata_repair_count()
    );
    println!("  link-repair files: {}", plan.link_repair_file_count());
    println!("  planned bytes: {}", plan.planned_bytes());

    if plan.is_empty() {
        println!("moves:");
        println!("  none");
        println!("git:");
        println!("  skipped: no vault changes");
        println!("summary:");
        println!("  no task blocks met the threshold; no vault changes made.");
        return 0;
    }

    let git_state = match prepare_git(&vault, child_env, &plan) {
        Ok(git_state) => git_state,
        Err(GitPrepareError::Command(exit_code)) => return exit_code,
    };

    println!("moves:");
    let mut archives_created = 0;
    let mut archives_updated = 0;
    for file in &plan.files {
        if file.writes_archive() {
            if file.task_count > 0 {
                println!(
                    "  {} -> {} ({} task blocks)",
                    file.relative_source_path.display(),
                    file.relative_archive_path.display(),
                    file.task_count
                );
            } else if file.source_metadata_updated {
                println!(
                    "  {} -> {} (source/archive metadata)",
                    file.relative_source_path.display(),
                    file.relative_archive_path.display()
                );
            } else if file.archive_link_repair_count > 0 {
                println!(
                    "  {} ({} Obsidian link repairs)",
                    file.relative_archive_path.display(),
                    file.archive_link_repair_count
                );
            } else {
                println!(
                    "  {} -> {} (archive metadata)",
                    file.relative_source_path.display(),
                    file.relative_archive_path.display()
                );
            }
        } else if file.source_link_repair_count > 0
            && !file.source_metadata_updated
        {
            println!(
                "  {} ({} Obsidian link repairs)",
                file.relative_source_path.display(),
                file.source_link_repair_count
            );
        } else {
            println!(
                "  {} -> {} (done_tasks metadata)",
                file.relative_source_path.display(),
                file.relative_archive_path.display()
            );
        }
        match apply_file_plan(&vault, file) {
            Ok(Some(ArchiveWrite::Created)) => archives_created += 1,
            Ok(Some(ArchiveWrite::Updated)) => archives_updated += 1,
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "{COMMAND_NAME}: failed to write vault changes: {error}"
                );
                return 1;
            }
        }
    }
    for repair in &plan.link_repairs {
        println!(
            "  {} ({} Obsidian link repairs, {} dependency metadata repairs)",
            repair.relative_path.display(),
            repair.link_count,
            repair.dependency_metadata_count
        );
        if let Err(error) = apply_link_repair_plan(&vault, repair) {
            eprintln!("{COMMAND_NAME}: failed to write vault changes: {error}");
            return 1;
        }
    }
    println!("git:");
    if let Err(exit_code) = finish_git(&vault, child_env, &git_state) {
        return exit_code;
    }
    println!("summary:");
    println!("  moved task blocks: {}", plan.total_task_count());
    println!(
        "  source files updated: {}",
        plan.source_file_update_count()
    );
    println!(
        "  source done_tasks updated: {}",
        plan.source_metadata_update_count()
    );
    println!(
        "  archive metadata repaired: {}",
        plan.archive_metadata_update_count()
    );
    println!("  moved block ids: {}", plan.moved_block_id_count());
    println!(
        "  moved block id renames: {}",
        plan.moved_block_id_rename_count()
    );
    println!("  Obsidian links repaired: {}", plan.link_repair_count());
    println!(
        "  dependency metadata repaired: {}",
        plan.dependency_metadata_repair_count()
    );
    println!(
        "  link-repair files updated: {}",
        plan.link_repair_file_count()
    );
    println!("  archive files created: {archives_created}");
    println!("  archive files updated: {archives_updated}");
    0
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
            _ if let Some(value) = text.strip_prefix("-t=") => {
                parsed.threshold = match parse_threshold_text(value) {
                    Ok(threshold) => threshold,
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
            Arg::new("threshold")
                .long("threshold")
                .short('t')
                .value_name("N")
                .value_parser(clap::value_parser!(usize))
                .help("Minimum completed/canceled task count per source note"),
        )
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::Help)
                .help("Show this help message and exit"),
        )
}

pub(crate) fn help_text() -> String {
    format!(
        "\
usage: {COMMAND_NAME} [-t|--threshold N]

Move done and canceled Bob task blocks from area and project notes into
done/ archives, link sources, repair archive metadata, and repair Obsidian
links to moved block ids. See docs/task-archive.md.

example:
  {COMMAND_NAME} --threshold 10

options:
  -h, --help       show this help message and exit
  -t, --threshold N
                   minimum completed/canceled task count per source note \
(default: {DEFAULT_THRESHOLD})"
    )
}

fn print_selection_report(plan: &CollectionPlan) {
    println!("holds:");
    if plan.holds.is_empty() {
        println!("  none");
    } else {
        for hold in &plan.holds {
            println!(
                "  {}:{} {}",
                hold.relative_path.display(),
                hold.line,
                hold.reason
            );
        }
    }
    println!("skips:");
    if plan.skip_counts.is_empty() {
        println!("  none");
    } else {
        for (reason, count) in &plan.skip_counts {
            println!("  {reason}: {count}");
        }
    }
    println!("grandfathered:");
    if plan.grandfathered.is_empty() {
        println!("  none");
    } else {
        for path in &plan.grandfathered {
            println!("  {}", path.display());
        }
    }
}

fn print_help() {
    println!("{}", help_text());
}
