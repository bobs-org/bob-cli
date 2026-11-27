//! Public CLI for formatting and inspecting prose notes.

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

use super::{
    adopt, format, format_with_section_numbers, inbound, render, vault,
    ProseContext, ProseStatus,
};
use crate::native::{env as bob_env, style::Styler};

const COMMAND_NAME: &str = "bob prose";

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
        Some(("adopt", sub_matches)) => run_adopt(sub_matches),
        Some(("extract", sub_matches)) => {
            super::convert::extract::run_extract(sub_matches)
        }
        Some(("format", sub_matches)) => run_format(sub_matches),
        Some(("inline", sub_matches)) => {
            super::convert::inline::run_inline(sub_matches)
        }
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
        .about("Format, inspect, extract, and inline prose notes")
        .long_about(
            "Format, inspect, extract, and inline prose notes (frontmatter type: \"[[prose]]\").\n\n\
             Every `bob prose` command reads or rewrites prose notes. `[[article]]` \
             notes still count. bob owns their section numbers (1. / 1.1 / 1.1.1) \
             and layout; formatting never changes a word. `extract` moves an authored \
             heading section into a prose note, and `inline` folds it back through its \
             unique link stub. Both preview before writing. Bare `bob prose` runs the \
             read-only `list` command.",
        )
        .after_help(
            "Examples:\n  bob prose                                      List prose notes\n  bob prose adopt blog                          Preview adopting a note\n  bob prose extract job_2026 -s \"Why I left Google?\"  Preview extracting a section\n  bob prose inline job_2026_why_i_left_google    Preview inlining it\n  bob prose format blog                         Format one prose note\n  bob prose outline blog                        Numbered outline with word counts\n\nSee docs/prose.md for the rules and JSON contract.",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .args_conflicts_with_subcommands(true)
        .subcommand_required(false)
        .subcommand(adopt_command())
        .subcommand(extract_command())
        .subcommand(format_command())
        .subcommand(inline_command())
        .subcommand(list_command())
        .subcommand(outline_command())
}

fn adopt_command() -> ClapCommand {
    ClapCommand::new("adopt")
        .about("Mark a note as a prose note; preview unless --write")
        .long_about(
            "Insert type: \"[[prose]]\" and plan the first format. \
             Previews by default with a diff and the numbers it will \
             replace. Pass --write to apply the change, or --stdin to \
             print a JSON plan for a buffer without writing.",
        )
        .after_help(
            "Examples:\n  bob prose adopt blog\n  bob prose adopt -w blog\n  cat blog.md | bob prose adopt -s blog -f json",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(stdin_arg().conflicts_with("write"))
        .arg(write_arg())
        .arg(note_arg_with_help(
            false,
            "Vault-relative Markdown path or unique note stem",
        ))
}

fn extract_command() -> ClapCommand {
    ClapCommand::new("extract")
        .about("Move a section into a new prose note, leaving a link stub")
        .long_about(
            "Extract one heading section from an area, project, or prose note \
             into a new prose note, leaving a link stub. Previews by default \
             with a unified diff per file; pass --write to apply through one \
             verified commit. Online writes need -b/--bob-dir and BOB_DIR to \
             resolve to the same vault; a mismatch refuses before locks or sync.",
        )
        .after_help(
            "Examples:\n  bob prose extract job_2026 -s \"Why I left Google?\"\n  bob prose extract job_2026 -l 31 -f json\n  bob prose extract job_2026 -s Intro -n custom_stem -w -o\n\nExit codes:\n  0 preview or write\n  2 refusal, usage error, or write failure",
        )
        .group(
            ArgGroup::new("selector")
                .args(["section", "line"])
                .required(true)
                .multiple(false),
        )
        .arg(bob_dir_arg())
        .arg(expect_arg())
        .arg(format_arg())
        .arg(line_arg())
        .arg(name_arg())
        .arg(offline_arg())
        .arg(section_arg())
        .arg(extract_write_arg())
        .arg(note_arg_with_help(
            false,
            "Vault-relative Markdown path or unique note stem to extract from",
        ))
}

fn inline_command() -> ClapCommand {
    ClapCommand::new("inline")
        .about("Fold a prose note back into its parent section")
        .long_about(
            "Inline a prose note into the unique section in its parent that links to it. \
             The note must have no child notes, unsupported frontmatter, or conflicting \
             heading or block identities. Previews by default with a unified diff per \
             file; pass --write to apply through one verified commit. Online writes need \
             -b/--bob-dir and BOB_DIR to resolve to the same vault; a mismatch refuses \
             before locks or sync.",
        )
        .after_help(
            "Examples:\n  bob prose inline job_2026_why_i_left_google\n  bob prose inline letter_to_maya -w -o -f json\n\nExit codes:\n  0 preview or write\n  2 refusal, usage error, or write failure",
        )
        .arg(bob_dir_arg())
        .arg(expect_arg())
        .arg(format_arg())
        .arg(offline_arg())
        .arg(inline_write_arg())
        .arg(note_arg_with_help(
            false,
            "Vault-relative prose-note path or unique note stem to inline",
        ))
}

fn format_command() -> ClapCommand {
    ClapCommand::new("format")
        .about("Number sections and tidy layout in prose notes")
        .long_about(
            "Format one or more prose notes. Writes by default; use --check \
             to report whether any note would change, --dry-run to show a \
             unified diff, --stdin to print a plan for text from stdin \
             without writing, or --section-numbers on|off to update numbering \
             and format in one transaction.",
        )
        .after_help(
            "Examples:\n  bob prose format blog\n  bob prose format -n off blog\n  bob prose format -a -c\n  bob prose format -d blog\n  cat blog.md | bob prose format -s blog -n off -f json",
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
        .arg(section_numbers_arg())
        .arg(stdin_arg())
        .arg(note_arg(true))
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List prose notes with sections, TODO stubs, and findings")
        .after_help(
            "Examples:\n  bob prose list\n  bob prose list -f json\n  bob prose list -b ~/bob",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
}

fn outline_command() -> ClapCommand {
    ClapCommand::new("outline")
        .about("Print a prose note's numbered section outline")
        .after_help(
            "Examples:\n  bob prose outline blog\n  bob prose outline blog -f json",
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
        .help("Format every prose note in the vault")
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
        .help("Exit 1 if any prose note would change; write nothing")
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

fn section_numbers_arg() -> Arg {
    Arg::new("section-numbers")
        .short('n')
        .long("section-numbers")
        .value_name("STATE")
        .value_parser(["on", "off"])
        .conflicts_with("all")
        .conflicts_with("check")
        .help("Set section numbering on or off and format in one write")
}

fn stdin_arg() -> Arg {
    Arg::new("stdin")
        .short('s')
        .long("stdin")
        .action(ArgAction::SetTrue)
        .help("Read the named note's text from stdin; print a plan and never write")
}

fn expect_arg() -> Arg {
    Arg::new("expect")
        .short('e')
        .long("expect")
        .value_name("FINGERPRINT")
        .help("Refuse with plan-changed when the replanned fingerprint differs; write nothing")
}

fn line_arg() -> Arg {
    Arg::new("line")
        .short('l')
        .long("line")
        .value_name("LINE")
        .value_parser(clap::value_parser!(usize))
        .help("1-based line inside the section to extract")
}

fn name_arg() -> Arg {
    Arg::new("name")
        .short('n')
        .long("name")
        .value_name("STEM")
        .value_parser(clap::builder::NonEmptyStringValueParser::new())
        .help("Override the derived prose-note stem")
}

fn offline_arg() -> Arg {
    Arg::new("offline")
        .short('o')
        .long("offline")
        .action(ArgAction::SetTrue)
        .help("Commit locally; skip vault sync before and after")
}

fn section_arg() -> Arg {
    Arg::new("section")
        .short('s')
        .long("section")
        .value_name("IDENTITY")
        .value_parser(clap::builder::NonEmptyStringValueParser::new())
        .help("Heading identity or nested Parent#Child path to extract")
}

fn extract_write_arg() -> Arg {
    Arg::new("write")
        .short('w')
        .long("write")
        .action(ArgAction::SetTrue)
        .help("Write the extraction through one verified commit; preview without this flag")
}

fn inline_write_arg() -> Arg {
    Arg::new("write")
        .short('w')
        .long("write")
        .action(ArgAction::SetTrue)
        .help("Write the inline through one verified commit; preview without this flag")
}

fn write_arg() -> Arg {
    Arg::new("write")
        .short('w')
        .long("write")
        .action(ArgAction::SetTrue)
        .help("Write the adopted prose note; preview without this flag")
}

fn note_arg(multiple: bool) -> Arg {
    note_arg_with_help(
        multiple,
        "Vault-relative prose-note path or unique note stem",
    )
}

fn note_arg_with_help(multiple: bool, help: &'static str) -> Arg {
    let arg = Arg::new("note")
        .value_name("NOTE")
        .value_parser(clap::builder::NonEmptyStringValueParser::new())
        .value_hint(clap::ValueHint::Other)
        .help(help);
    if multiple {
        arg.num_args(1..)
    } else {
        arg.num_args(1).required(true)
    }
}

fn run_adopt(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let write = matches.get_flag("write");
    let from_stdin = matches.get_flag("stdin");
    let output_format = output_format(matches);
    let note = matches
        .get_one::<String>("note")
        .expect("adopt requires NOTE");

    if from_stdin && write {
        return usage_error(
            adopt_command(),
            "--stdin never writes and cannot be combined with --write",
        );
    }

    let path = match vault::resolve_note(&bob_dir, note) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{COMMAND_NAME}: {message}");
            return 2;
        }
    };
    let source = if from_stdin {
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
                eprintln!("{COMMAND_NAME}: read {}: {error}", path.display());
                return 2;
            }
        }
    };

    let mut plans = vec![adopt(&source, &ProseContext::new(&path))];
    let index = match inbound::attach(&bob_dir, &mut plans, from_stdin) {
        Ok(index) => index,
        Err(error) => return print_vault_error(error),
    };
    let plan_failed = plans.iter().any(|plan| {
        matches!(plan.status, ProseStatus::Skipped | ProseStatus::Error)
    });

    if !plan_failed && write && !from_stdin {
        if let Err(message) = vault::preflight_plans(&bob_dir, &mut plans) {
            if output_format == OutputFormat::Json {
                render::format_json(plans);
            }
            eprintln!("{COMMAND_NAME}: {message}");
            return 2;
        }
        if plans.iter().all(|plan| plan.status == ProseStatus::Changed) {
            inbound::fold_into_formatted(
                &mut plans,
                std::slice::from_ref(&source),
                &index,
            );
            if let Err(message) = vault::write_plans(&bob_dir, &mut plans) {
                if output_format == OutputFormat::Json {
                    render::format_json(plans);
                }
                eprintln!("{COMMAND_NAME}: {message}");
                return 2;
            }
            for reason in inbound::write_others(&bob_dir, &plans, &index) {
                eprintln!("{COMMAND_NAME}: {reason}");
            }
        }
    }

    match output_format {
        OutputFormat::Json => render::format_json(plans.clone()),
        OutputFormat::Human => {
            render::adopt_human(&plans[0], &source, &Styler::detect(), write)
        }
    }

    let plan_failed = plans.iter().any(|plan| {
        matches!(plan.status, ProseStatus::Skipped | ProseStatus::Error)
    });
    if plan_failed {
        2
    } else {
        0
    }
}

fn run_format(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let all = matches.get_flag("all");
    let check = matches.get_flag("check");
    let dry_run = matches.get_flag("dry-run");
    let from_stdin = matches.get_flag("stdin");
    let section_numbers = matches
        .get_one::<String>("section-numbers")
        .map(|state| state == "on");
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
        let prose = match vault::discover_prose(&bob_dir) {
            Ok(prose) => prose,
            Err(error) => return print_vault_error(error),
        };
        (
            prose
                .iter()
                .map(|prose| prose.text.clone())
                .collect::<Vec<_>>(),
            prose
                .into_iter()
                .map(|prose| prose.path)
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

    let mut plans = sources
        .iter()
        .zip(&paths)
        .map(|(source, path)| {
            let context = ProseContext::new(path);
            match section_numbers {
                Some(enabled) => {
                    format_with_section_numbers(source, &context, enabled)
                }
                None => format(source, &context),
            }
        })
        .collect::<Vec<_>>();
    let index = match inbound::attach(&bob_dir, &mut plans, all || from_stdin) {
        Ok(index) => index,
        Err(error) => return print_vault_error(error),
    };
    if !check && !dry_run && !from_stdin {
        if let Err(message) = vault::preflight_plans(&bob_dir, &mut plans) {
            if output_format == OutputFormat::Json {
                render::format_json(plans);
            }
            eprintln!("{COMMAND_NAME}: {message}");
            return 2;
        }
        inbound::fold_into_formatted(&mut plans, &sources, &index);
        if let Err(message) = vault::write_plans(&bob_dir, &mut plans) {
            if output_format == OutputFormat::Json {
                render::format_json(plans);
            }
            eprintln!("{COMMAND_NAME}: {message}");
            return 2;
        }
        for reason in inbound::write_others(&bob_dir, &plans, &index) {
            eprintln!("{COMMAND_NAME}: {reason}");
        }
    }

    let plan_failed = plans.iter().any(|plan| {
        matches!(plan.status, ProseStatus::Skipped | ProseStatus::Error)
    });
    match output_format {
        OutputFormat::Json => render::format_json(plans.clone()),
        OutputFormat::Human => render::format_human(
            &plans,
            &sources,
            &bob_dir,
            &index,
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
            .any(|plan| matches!(plan.status, ProseStatus::Changed))
    {
        return 1;
    }
    0
}

fn run_list(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let prose = match vault::discover_prose(&bob_dir) {
        Ok(prose) => prose,
        Err(error) => return print_vault_error(error),
    };
    let plans = prose
        .iter()
        .map(|prose| format(&prose.text, &ProseContext::new(&prose.path)))
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
    let plan = format(&source, &ProseContext::new(&path));
    if matches!(plan.status, ProseStatus::Skipped | ProseStatus::Error) {
        if output_format(matches) == OutputFormat::Json {
            render::outline_json(&plan);
        }
        let message = plan
            .reason
            .as_ref()
            .map(|reason| reason.message.as_str())
            .unwrap_or("prose note could not be formatted");
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
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
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
