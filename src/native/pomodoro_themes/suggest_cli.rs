//! Read-only explanations of the shared task-theme suggestion rule.
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use clap::{builder::OsStringValueParser, Arg, ArgAction, Command};
use serde::Serialize;
use serde_json::json;

use super::{
    suggest::{self, SuggestWarning, ThemeSuggestion},
    Catalog, ThemeRef,
};
use crate::native::{
    capture,
    capture_language::is_block_id,
    config, env as bob_env, note_tasks,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    style::Styler,
};

const COMMAND: &str = "bob types suggest";

pub(crate) fn build_cli() -> Command {
    Command::new(COMMAND)
        .about("Explain the suggested type for a task or capture route")
        .long_about("Explain a task's suggested type without changing the vault.\n\nUse route:block-id for an existing task, or a bare route for a new task's\ndefault. Task fields take precedence over reading and note\ndefaults. Hidden and unknown types are reported; closed tasks have no suggestion.")
        .disable_help_flag(true)
        .arg(Arg::new("target").value_name("TARGET").required(true)
            .help("Capture route or route:block-id (without @ or ^)"))
        .arg(Arg::new("bob-dir").long("bob-dir").short('b').value_name("DIR")
            .value_parser(OsStringValueParser::new())
            .help("Bob vault root; defaults to BOB_DIR or ~/bob"))
        .arg(Arg::new("format").long("format").short('f').value_name("FORMAT")
            .value_parser(["human", "json"]).default_value("human")
            .help("Output format: human or json"))
        .arg(Arg::new("help").long("help").short('h').action(ArgAction::Help).help("Show help"))
        .after_help("Examples:\n  bob types suggest dev\n  bob types suggest dev:build -f json\n\nEnvironment:\n  BOB_CONFIG_FILE  Exact Bob config file\n  BOB_DIR          Vault root when --bob-dir is omitted\n  BOB_NOW          Local datetime override\n  NO_COLOR         Disable colored output\n\nExit codes:\n  0  explanation (including no suggestion or a closed task)\n  1  target lookup or I/O failure\n  2  usage or config error\n\nSee docs/task-types.md for the suggestion contract.")
}

#[derive(Serialize)]
struct Target {
    route: String,
    block_id: Option<String>,
    path: String,
    line: Option<usize>,
    new_task: bool,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    ok: bool,
    target: Target,
    theme_suggestion: Option<ThemeSuggestion>,
    warnings: Vec<SuggestWarning>,
}

struct Error {
    code: &'static str,
    message: String,
    exit: i32,
}

impl Error {
    fn io(message: impl Into<String>) -> Self {
        Self {
            code: "io",
            message: message.into(),
            exit: 1,
        }
    }
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: "usage",
            message: message.into(),
            exit: 2,
        }
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let matches = match build_cli().try_get_matches_from(
        std::iter::once(OsString::from(COMMAND)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    let json_output =
        matches.get_one::<String>("format").map(String::as_str) == Some("json");
    let root = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let target = matches
        .get_one::<String>("target")
        .expect("required target");
    let (output, exit) =
        match explain(&root, target) {
            Ok(report) => (
                if json_output {
                    serde_json::to_string(&report).expect("report JSON")
                } else {
                    human_report(&report)
                },
                0,
            ),
            Err(error) => {
                if !json_output {
                    eprintln!("{COMMAND}: {}", error.message);
                    return error.exit;
                }
                (json!({"schema_version": 1, "ok": false, "code": error.code,
                "message": error.message}).to_string(), error.exit)
            }
        };
    if let Err(error) = writeln!(io::stdout().lock(), "{output}") {
        if error.kind() == io::ErrorKind::BrokenPipe {
            return exit;
        }
        eprintln!("{COMMAND}: write output: {error}");
        return 1;
    }
    exit
}

fn explain(root: &Path, input: &str) -> Result<Report, Error> {
    let (route, block_id) =
        match input.split_once(':') {
            Some((route, id)) if is_block_id(id) => (route, Some(id)),
            Some(_) => return Err(Error::usage(
                "TARGET must be route or route:block-id with a valid block ID",
            )),
            None => (input, None),
        };
    if !capture::is_route_token(route) {
        return Err(Error::usage(
            "route must contain only A-Z, a-z, 0-9, '_' or '-'",
        ));
    }
    let route = route.to_ascii_lowercase();
    let path = capture::route_label(&route);
    let root = root
        .canonicalize()
        .map_err(|error| Error::io(format!("read vault: {error}")))?;
    let target = root
        .join(&path)
        .canonicalize()
        .map_err(|error| Error::io(format!("read target {path}: {error}")))?;
    if !target.starts_with(&root) {
        return Err(Error::usage("target resolves outside the Bob vault"));
    }
    let contents = fs::read_to_string(&target)
        .map_err(|error| Error::io(format!("read target {path}: {error}")))?;
    let settings = note_tasks::read_settings(&root);
    let scan = note_tasks::scan(&contents, &settings);
    let task = block_id
        .map(|id| scan.lookup_capture_task(&route, id))
        .transpose()
        .map_err(Error::io)?;
    let cfg =
        config::load_plan_config(&config::config_path()).map_err(|error| {
            Error {
                code: "config",
                message: error.to_string(),
                exit: 2,
            }
        })?;
    let catalog =
        Catalog::load(&root, bob_env::current_datetime().date(), false)
            .map_err(|error| {
                Error::io(format!("read type catalog: {error}"))
            })?;
    let mut source = HomeResolver::new(DiskHomeSource::new(&root));
    let result = match task {
        Some(task) => suggest::suggest_for_task(
            &mut source,
            &catalog,
            &cfg,
            Path::new(&path),
            contents
                .lines()
                .nth(task.line_index)
                .expect("scanned task line"),
        ),
        None => suggest::suggest_for_new_task(
            &mut source,
            &catalog,
            &cfg,
            Path::new(&path),
        ),
    };
    Ok(Report {
        schema_version: 1,
        ok: true,
        target: Target {
            route,
            block_id: block_id.map(str::to_string),
            path,
            line: task.map(|task| task.line_index + 1),
            new_task: task.is_none(),
        },
        theme_suggestion: result.suggestion,
        warnings: result.warnings,
    })
}

fn swatch(theme: &ThemeRef, styler: &Styler) -> String {
    let mark = if !theme.known {
        "◌"
    } else if theme.hue.is_none() {
        "□"
    } else {
        "■"
    };
    let mark = if theme.known && styler.is_color() {
        match theme.hue {
            Some(hue) => styler.paint(&format!("38;5;{}", hue.xterm()), mark),
            None => styler.dim(mark),
        }
    } else {
        mark.to_string()
    };
    format!("{mark} {}", theme.name)
}

fn human_report(report: &Report) -> String {
    let styler = Styler::detect();
    let mut lines = Vec::new();
    match &report.theme_suggestion {
        Some(suggestion) => {
            lines.push(format!(
                "{}  suggested · {}",
                swatch(&suggestion.theme, &styler),
                suggestion.label
            ));
            if let Some(from) = &suggestion.from {
                lines.push(format!("  declaring note: {}", from.path));
                lines.push(format!("  chain: {}", from.chain.join(" → ")));
            }
            if let Some(inherited) = &suggestion.inherited {
                lines.push(match inherited {
                    Some(inherited) => format!(
                        "  without it: {} {}",
                        swatch(&inherited.theme, &styler),
                        inherited.label
                    ),
                    None => "  without it: none (no default here)".to_string(),
                });
            }
        }
        None => lines.push("No suggested type".to_string()),
    }
    for warning in &report.warnings {
        lines.push(format!(
            "  warning: {}{}",
            warning.code,
            warning
                .path
                .as_ref()
                .map(|path| format!(" · {path}"))
                .unwrap_or_default()
        ));
    }
    lines.join("\n")
}
