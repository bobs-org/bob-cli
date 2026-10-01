use std::{ffi::OsString, iter};

use super::env as bob_env;

mod cli;
mod error;
mod eval;
mod functions;
mod index;
mod lexer;
mod native;
mod obsidian;
mod output;
mod parser;
mod render;
mod sources;
mod tasks;
#[cfg(test)]
mod tests;
mod value;
mod vault;

use cli::*;
use error::*;
use eval::*;
use functions::*;
use index::*;
use lexer::*;
use native::*;
use obsidian::*;
use output::*;
use parser::*;
use render::*;
use sources::*;
use value::*;
use vault::*;

pub(crate) use error::DataviewError;
use sources::{
    collect_native_markdown_paths, native_frontmatter_block,
    native_link_target, normalize_note_path, note_stem, unquote_native_scalar,
};
#[cfg(test)]
pub(crate) use tasks::{parse_details, TaskDetails};
pub(crate) use tasks::{
    query_matching_descriptions, query_rich_tasks, read_task_format,
    scan_all_rich_tasks, tasks_fingerprint, RichTask, TaskFormat, NEXT_QUERY,
    OPEN_QUERY, PENDING_QUERY, READY_QUERY,
};

const COMMAND_NAME: &str = "bob query";
const ENV_OBSIDIAN_COMMAND: &str = "BOB_DATAVIEW_OBSIDIAN_COMMAND";
const ENV_VAULT: &str = "BOB_DATAVIEW_VAULT";
const RESULT_PREFIX: &str = "BOB_DATAVIEW_RESULT\t";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let request = match Request::from_matches(&matches, &mut command) {
        Ok(request) => request,
        Err(error) => return print_clap_error(error),
    };

    match run_request(&request) {
        Ok(()) => 0,
        Err(error) => {
            error.report();
            error.exit_code()
        }
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

fn run_request(request: &Request) -> Result<(), DataviewError> {
    match request.engine {
        Engine::Obsidian => run_obsidian(request),
        Engine::Native => run_native(request),
    }
}

fn run_obsidian(request: &Request) -> Result<(), DataviewError> {
    let eval_request = request.obsidian_eval_request()?;

    let javascript = build_obsidian_javascript(&eval_request)?;
    let output = run_obsidian_eval(&request.vault, &javascript)?;
    let engine_output = parse_protocol_stdout(&output.stdout)?;
    emit_engine_output(request, engine_output)
}

fn run_native(request: &Request) -> Result<(), DataviewError> {
    let vault = NativeVault::read(
        &request.vault.bob_dir,
        request.vault.origin.as_deref(),
    )?;
    match &request.query {
        QueryInput::Source(source) => {
            let source = NativeSourceExpr::parse(source)?;
            let output = vault.evaluate_source(&source);
            emit_engine_output(request, output)
        }
        QueryInput::Dql(input) => {
            let query = NativeQuery::parse(&input.read_query()?)?;
            if request.format == OutputFormat::Markdown {
                let settings =
                    NativeMarkdownSettings::read(&request.vault.bob_dir);
                let output = vault.evaluate_markdown(&query, &settings)?;
                emit_engine_output(request, output)
            } else {
                let output = vault.evaluate(&query);
                emit_native_output(request, output)
            }
        }
        QueryInput::Tasks(input) => tasks::run(
            &request.vault.bob_dir,
            request.vault.origin.as_deref(),
            &input.read_query()?,
            request.format,
        ),
        QueryInput::TasksNote(path) => {
            tasks::run_note(&request.vault.bob_dir, path, request.format)
        }
    }
}
