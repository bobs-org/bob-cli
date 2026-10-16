//! Public CLI for formatting and inspecting article notes.

use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    iter,
    path::PathBuf,
};

use clap::{
    builder::OsStringValueParser, error::ErrorKind, Arg, ArgAction, ArgGroup,
    ArgMatches, Command as ClapCommand,
};

use super::{format, render, vault, ArticleContext, ArticleStatus};
use crate::native::{env as bob_env, style::Styler};

const COMMAND_NAME: &str = "bob articles";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    match matches.subcommand() {
        Some(("format", sub_matches)) => run_format(sub_matches),
        Some(("list", sub_matches)) => run_list(sub_matches),
        Some(("outline", sub_matches)) => run_outline(sub_matches),
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        None => run_list(&matches),
    }
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Format and inspect article notes (frontmatter type: [[article]])")
        .long_about(
            "Format and inspect article notes (frontmatter type: \"[[article]]\").\n\n\
             Every `bob articles` command reads or rewrites notes whose type is \
             `[[article]]`. bob owns their section numbers (1. / 1.1 / 1.1.1) \
             and layout; formatting never changes a word. Bare `bob articles` \
             runs the read-only `list` command.",
        )
        .after_help(
            "Examples:\n  bob articles                         List article notes\n  bob articles format blog              Format one article\n  bob articles format -a -c             Check every article without writing\n  bob articles outline blog              Show its numbered section outline\n\nSee docs/articles.md for the rules and JSON contract.",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .args_conflicts_with_subcommands(true)
        .subcommand_required(false)
        .subcommand(format_command())
        .subcommand(list_command())
        .subcommand(outline_command())
}

fn format_command() -> ClapCommand {
    ClapCommand::new("format")
        .about("Number sections and tidy layout in article notes")
        .long_about(
            "Format one or more article notes. Writes by default; use --check \
             to report whether any note would change, --dry-run to show a \
             unified diff, or --stdin to print a plan for text from stdin \
             without writing.",
        )
        .after_help(
            "Examples:\n  bob articles format blog\n  bob articles format -a -c\n  bob articles format -d blog\n  cat blog.md | bob articles format -s blog -f json",
        )
        .group(
            ArgGroup::new("target")
                .args(["note", "all"])
                .required(true)
                .multiple(false),
        )
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(check_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(stdin_arg())
        .arg(note_arg(true))
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List article notes with sections, TODO stubs, and findings")
        .after_help(
            "Examples:\n  bob articles list\n  bob articles list -f json\n  bob articles list -b ~/bob",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
}

fn outline_command() -> ClapCommand {
    ClapCommand::new("outline")
        .about("Print an article's numbered section outline")
        .after_help(
            "Examples:\n  bob articles outline blog\n  bob articles outline blog -f json",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(note_arg(false))
}

fn all_arg() -> Arg {
    Arg::new("all")
        .short('a')
        .long("all")
        .action(ArgAction::SetTrue)
        .help("Format every article note in the vault")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .short('b')
        .long("bob-dir")
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn check_arg() -> Arg {
    Arg::new("check")
        .short('c')
        .long("check")
        .action(ArgAction::SetTrue)
        .help("Exit 1 if any article would change; write nothing")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .short('d')
        .long("dry-run")
        .action(ArgAction::SetTrue)
        .conflicts_with("check")
        .help("Print a colored unified diff without writing")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .short('f')
        .long("format")
        .value_name("FORMAT")
        .default_value("human")
        .value_parser(["human", "json"])
        .help("Output format: human or json")
}

fn stdin_arg() -> Arg {
    Arg::new("stdin")
        .short('s')
        .long("stdin")
        .action(ArgAction::SetTrue)
        .help("Read the named note's text from stdin; print a plan and never write")
}

fn note_arg(multiple: bool) -> Arg {
    let arg = Arg::new("note")
        .value_name("NOTE")
        .value_parser(clap::builder::NonEmptyStringValueParser::new())
        .value_hint(clap::ValueHint::Other)
        .help("Vault-relative article path or unique note stem");
    if multiple {
        arg.num_args(1..)
    } else {
        arg.num_args(1).required(true)
    }
}

fn run_format(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let all = matches.get_flag("all");
    let check = matches.get_flag("check");
    let dry_run = matches.get_flag("dry-run");
    let from_stdin = matches.get_flag("stdin");
    let output_format = output_format(matches);
    let requested = matches
        .get_many::<String>("note")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    if from_stdin && (all || requested.len() != 1) {
        return usage_error(
            format_command(),
            "--stdin requires exactly one NOTE and cannot be used with --all",
        );
    }
    if from_stdin && dry_run {
        return usage_error(
            format_command(),
            "--stdin already prints a non-writing plan and cannot be combined with --dry-run",
        );
    }

    let (sources, paths) = if all {
        let articles = match vault::discover_articles(&bob_dir) {
            Ok(articles) => articles,
            Err(error) => return print_vault_error(error),
        };
        (
            articles
                .iter()
                .map(|article| article.text.clone())
                .collect::<Vec<_>>(),
            articles
                .into_iter()
                .map(|article| article.path)
                .collect::<Vec<_>>(),
        )
    } else {
        let mut sources = Vec::with_capacity(requested.len());
        let mut paths = Vec::with_capacity(requested.len());
        for note in &requested {
            let path = match vault::resolve_note(&bob_dir, note) {
                Ok(path) => path,
                Err(message) => {
                    eprintln!("{COMMAND_NAME}: {message}");
                    return 2;
                }
            };
            let text = if from_stdin {
                let mut text = String::new();
                if let Err(error) = io::stdin().read_to_string(&mut text) {
                    eprintln!("{COMMAND_NAME}: read stdin: {error}");
                    return 2;
                }
                text
            } else {
                match fs::read_to_string(bob_dir.join(&path)) {
                    Ok(text) => text,
                    Err(error) => {
                        eprintln!(
                            "{COMMAND_NAME}: read {}: {error}",
                            path.display()
                        );
                        return 2;
                    }
                }
            };
            paths.push(path);
            sources.push(text);
        }
        (sources, paths)
    };

    let plans = sources
        .iter()
        .zip(&paths)
        .map(|(source, path)| format(source, &ArticleContext::new(path)))
        .collect::<Vec<_>>();
    let plan_failed = plans.iter().any(|plan| {
        matches!(plan.status, ArticleStatus::Skipped | ArticleStatus::Error)
    });

    if !plan_failed && !check && !dry_run && !from_stdin {
        if let Err(message) = vault::write_plans(&bob_dir, &plans) {
            if output_format == OutputFormat::Json {
                render::format_json(plans);
            }
            eprintln!("{COMMAND_NAME}: {message}");
            return 2;
        }
    }

    match output_format {
        OutputFormat::Json => render::format_json(plans.clone()),
        OutputFormat::Human => render::format_human(
            &plans,
            &sources,
            &Styler::detect(),
            check,
            dry_run,
        ),
    }

    if plan_failed {
        return 2;
    }
    if check
        && plans
            .iter()
            .any(|plan| matches!(plan.status, ArticleStatus::Changed))
    {
        return 1;
    }
    0
}

fn run_list(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let articles = match vault::discover_articles(&bob_dir) {
        Ok(articles) => articles,
        Err(error) => return print_vault_error(error),
    };
    let plans = articles
        .iter()
        .map(|article| {
            format(&article.text, &ArticleContext::new(&article.path))
        })
        .collect::<Vec<_>>();
    match output_format(matches) {
        OutputFormat::Human => {
            render::list_human(&plans, &bob_dir, &Styler::detect());
        }
        OutputFormat::Json => render::list_json(plans),
    }
    0
}

fn run_outline(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let note = matches
        .get_one::<String>("note")
        .expect("outline requires NOTE");
    let path = match vault::resolve_note(&bob_dir, note) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{COMMAND_NAME}: {message}");
            return 2;
        }
    };
    let source = match fs::read_to_string(bob_dir.join(&path)) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("{COMMAND_NAME}: read {}: {error}", path.display());
            return 2;
        }
    };
    let plan = format(&source, &ArticleContext::new(&path));
    if matches!(plan.status, ArticleStatus::Skipped | ArticleStatus::Error) {
        if output_format(matches) == OutputFormat::Json {
            render::outline_json(&plan);
        }
        let message = plan
            .reason
            .as_ref()
            .map(|reason| reason.message.as_str())
            .unwrap_or("article could not be formatted");
        eprintln!("{COMMAND_NAME}: {}: {message}", path.display());
        return 2;
    }
    match output_format(matches) {
        OutputFormat::Human => render::outline_human(&plan, &Styler::detect()),
        OutputFormat::Json => render::outline_json(&plan),
    }
    0
}

fn output_format(matches: &ArgMatches) -> OutputFormat {
    match matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("human")
    {
        "json" => OutputFormat::Json,
        _ => OutputFormat::Human,
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
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

fn usage_error(mut command: ClapCommand, message: &str) -> i32 {
    let error = command.error(ErrorKind::ArgumentConflict, message.to_string());
    print_clap_error(error)
}

fn print_vault_error(error: vault::VaultError) -> i32 {
    eprintln!("{COMMAND_NAME}: {error}");
    1
}
