use std::{
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{capture, env as bob_env, style::Styler};

const COMMAND_NAME: &str = "bob capture-sections";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match CaptureSectionsRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_sections_error(error, output_format),
    };

    match list_capture_sections(&request.bob_dir, &request.route) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_sections_error(error, output_format),
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
        .about("List the non-Tasks sections of a capture note")
        .long_about(
            "List the existing non-Tasks Markdown sections of one routable Bob \
note.\n\n\
The command is read-only and reports every ATX heading level (H1-H6) that Bob's \
bullet capture can target, in document order. Missing notes are not errors; \
they return an empty list so picker callers can skip the section chooser.",
        )
        .after_help(
            "Examples:\n  bob capture-sections --route cash --format json\n  bob capture-sections --route cash\n  bob capture-sections -r cash -f json\n  bob capture-sections -b ~/bob -r project-alpha",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(route_arg())
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

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn route_arg() -> Arg {
    Arg::new("route")
        .long("route")
        .short('r')
        .value_name("NAME")
        .help("Route/name of the capture note whose sections to list")
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureSectionsRequest {
    bob_dir: PathBuf,
    route: String,
}

impl CaptureSectionsRequest {
    fn from_matches(
        matches: &ArgMatches,
    ) -> Result<Self, CaptureSectionsError> {
        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            route: route_from_matches(matches)?,
        })
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn route_from_matches(
    matches: &ArgMatches,
) -> Result<String, CaptureSectionsError> {
    let Some(route) = matches.get_one::<String>("route") else {
        return Err(CaptureSectionsError::usage("--route is required"));
    };
    normalize_route(route)
}

fn normalize_route(route: &str) -> Result<String, CaptureSectionsError> {
    if capture::is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }

    Err(CaptureSectionsError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureSectionsResult {
    ok: bool,
    route: String,
    count: usize,
    sections: Vec<capture::SectionHeading>,
}

fn list_capture_sections(
    bob_dir: &Path,
    route: &str,
) -> Result<CaptureSectionsResult, CaptureSectionsError> {
    let target = bob_dir.join(capture::route_label(route));
    let contents = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(fs_error("read target", &target, error));
        }
    };
    let sections = capture::non_tasks_section_headings(&contents);

    Ok(CaptureSectionsResult {
        ok: true,
        route: route.to_string(),
        count: sections.len(),
        sections,
    })
}

fn print_success(result: &CaptureSectionsResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureSectionsResult) {
    let styler = Styler::detect();
    let route_label = capture::route_label(&result.route);
    println!(
        "Capture sections {} {}",
        styler.separator(),
        styler.cyan(&route_label)
    );
    println!();

    if result.sections.is_empty() {
        println!("  No non-Tasks sections found.");
    } else {
        for section in &result.sections {
            let level = format!("H{}", section.level);
            println!(
                "  {}  {}",
                styler.dim(&level),
                styler.cyan(&section.title)
            );
        }
    }

    println!();
    println!(
        "{} {}",
        result.count,
        plural(result.count, "section", "sections")
    );
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn success_json(result: &CaptureSectionsResult) -> String {
    serde_json::to_string(result).expect("serialize capture sections result")
}

fn print_sections_error(
    error: CaptureSectionsError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

fn fs_error(
    action: &str,
    path: &Path,
    error: io::Error,
) -> CaptureSectionsError {
    CaptureSectionsError::io(format!("{action} {}: {error}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureSectionsError {
    kind: CaptureSectionsErrorKind,
    message: String,
}

impl CaptureSectionsError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureSectionsErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureSectionsErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureSectionsErrorKind {
    Usage,
    Io,
}

impl CaptureSectionsErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn route_validation_lowercases_valid_route() {
        assert_eq!(normalize_route("Cash-Flow").as_deref(), Ok("cash-flow"));

        let error = normalize_route("../bad").expect_err("invalid route");
        assert_eq!(error.kind, CaptureSectionsErrorKind::Usage);
    }

    #[test]
    fn missing_file_returns_empty_sections() {
        let temp = TempDir::new("bob-cli-capture-sections-missing");
        fs::create_dir_all(temp.path()).expect("create temp vault");

        let result = list_capture_sections(temp.path(), "cash")
            .expect("missing note is not an error");
        assert_eq!(
            result,
            CaptureSectionsResult {
                ok: true,
                route: "cash".to_string(),
                count: 0,
                sections: Vec::new(),
            }
        );
    }

    #[test]
    fn existing_file_lists_non_tasks_sections_in_order() {
        let temp = TempDir::new("bob-cli-capture-sections-existing");
        write_file(
            &temp.path().join("cash.md"),
            concat!(
                "---\n",
                "## Ignored\n",
                "---\n",
                "# Cash\n",
                "```md\n",
                "## Ignored\n",
                "```\n",
                "## Tasks\n",
                "### Ideas\n",
                "###### Log\n",
            ),
        );

        let result =
            list_capture_sections(temp.path(), "cash").expect("list sections");
        assert_eq!(result.count, 3);
        assert_eq!(
            result.sections,
            vec![
                capture::SectionHeading {
                    title: "Cash".to_string(),
                    level: 1,
                },
                capture::SectionHeading {
                    title: "Ideas".to_string(),
                    level: 3,
                },
                capture::SectionHeading {
                    title: "Log".to_string(),
                    level: 6,
                },
            ]
        );
    }

    #[test]
    fn json_success_shape_is_stable() {
        let result = CaptureSectionsResult {
            ok: true,
            route: "cash".to_string(),
            count: 2,
            sections: vec![
                capture::SectionHeading {
                    title: "Ideas".to_string(),
                    level: 2,
                },
                capture::SectionHeading {
                    title: "Log".to_string(),
                    level: 3,
                },
            ],
        };

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["route"], "cash");
        assert_eq!(value["count"], 2);
        assert_eq!(value["sections"][0]["title"], "Ideas");
        assert_eq!(value["sections"][0]["level"], 2);
        assert_eq!(value["sections"][1]["title"], "Log");
        assert_eq!(value["sections"][1]["level"], 3);
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }
}
