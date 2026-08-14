use std::{
    ffi::OsString,
    io::{self, IsTerminal, Read},
    iter,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture_language::{self, Diagnostic, EditorMode, Need, Severity, Span},
    style::Styler,
};

const COMMAND_NAME: &str = "bob capture-parse";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let raw_text = match raw_text_from_matches(&matches) {
        Ok(raw_text) => raw_text,
        Err(error) => return print_parse_error(&error, output_format),
    };
    if capture_language::normalize_task_text(&raw_text).is_empty() {
        return print_parse_error(
            &capture_language::missing_text_error(),
            output_format,
        );
    }

    let result = CaptureParseResult::new(raw_text);
    print_success(&result, output_format);
    0
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

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Explain what in-progress capture text currently means")
        .long_about(
            "Report the authoritative capture grammar's view of TEXT.\n\n\
The command is purely lexical and completely read-only: it never opens the \
vault, never reads the clipboard, never touches the filesystem, and takes no \
--bob-dir. Running it with a nonexistent BOB_DIR and a '%' clipboard marker \
still succeeds.\n\n\
It reports the normalized body, the overall capture mode, the resolved route, \
section, and block ID, which parts a picker still has to supply, the UTF-8 \
byte spans of every recognized token, every authored sub-bullet's normalized \
body, and structured diagnostics. Byte offsets index the original TEXT \
before whitespace normalization, are half-open [start, end), never overlap, \
and always land on a character boundary.\n\n\
TEXT accepts the same multi-line authored-bullet draft 'bob capture' does: \
the first physical line is the parent, and later flat '-'/'*'/'+' lines \
become authored children. Incomplete interactive markers are valid input, \
not errors, on any line: '@', '@#', '@#Ideas', '@route#', '@:', '@route:', \
'@^', '@route^', and the legacy '@!' aliases all report mode 'incomplete' \
plus what they still need. An invalid marker component, a malformed \
continuation line, an item emptied by marker removal, or a duplicate \
capture-wide marker across lines becomes a diagnostic, so live editors keep \
a usable parse while 'bob capture' keeps its strict execution errors.\n\n\
Only a missing TEXT or a bad flag is an error; every other input succeeds. \
If TEXT is omitted and stdin is piped, it reads the complete piped stdin \
stream.",
        )
        .after_help(
            "Examples:\n  bob capture-parse 'Call bank @Cash^'\n  bob capture-parse -f json -- 'jot idea @notes#Ideas'\n  echo 'Do work @dev:focus-123' | bob capture-parse -f json\n  printf 'Parent\\n- first child\\n- second child\\n' | bob capture-parse\n\nModes:\n  task, bullet, pomodoro_task, sub_bullet, incomplete\n\nNeeds:\n  route, section, pomodoro_id, task",
        )
        .disable_help_flag(true)
        .arg(format_arg())
        .arg(help_arg())
        .arg(text_arg())
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

fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(OsStringValueParser::new())
        .help("Capture text; multiple args are joined with spaces")
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

/// Mirror `bob capture`'s convention: join every TEXT argument with spaces,
/// or read the complete piped stdin stream when TEXT is omitted, so a
/// multi-line authored-bullet draft survives a pipe exactly like it does as
/// a single argv value.
fn raw_text_from_matches(matches: &ArgMatches) -> Result<String, String> {
    if let Some(values) = matches.get_many::<OsString>("text") {
        return Ok(values
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "));
    }

    if io::stdin().is_terminal() {
        return Ok(String::new());
    }

    let mut text = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut text)
        .map_err(|error| format!("read stdin: {error}"))?;
    Ok(text)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureParseResult {
    ok: bool,
    schema_version: u32,
    input: String,
    body: String,
    mode: EditorMode,
    route: Option<String>,
    section: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
    spans: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    /// Normalized authored-child bodies (source list marker and capture
    /// markers already removed) of every valid later physical line, in
    /// source order. Additive to schema version 1: omitted when empty, so
    /// an ordinary single-line draft's JSON shape is unchanged. These are
    /// semantic parse bodies, not rendered Markdown -- they carry no
    /// target-selected indentation or `- ` marker, unlike `bob capture`'s
    /// own `sub_bullets` output field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sub_bullets: Vec<String>,
}

impl CaptureParseResult {
    fn new(input: String) -> Self {
        let parse = capture_language::parse_for_editor(&input);
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            input,
            body: parse.body,
            mode: parse.mode,
            route: parse.route,
            section: parse.section,
            block_id: parse.block_id,
            needs: parse.needs,
            spans: parse.spans,
            diagnostics: parse.diagnostics,
            sub_bullets: parse.sub_bullets,
        }
    }
}

fn print_success(result: &CaptureParseResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureParseResult) {
    let styler = Styler::detect();
    print_human_success_with_styler(result, &styler);
}

fn print_human_success_with_styler(
    result: &CaptureParseResult,
    styler: &Styler,
) {
    println!(
        "Capture parse {} {}",
        styler.separator(),
        styler.cyan(result.mode.label())
    );
    println!();
    print_field(styler, "body", &result.body);
    if let Some(route) = result.route.as_deref() {
        print_field(styler, "route", route);
    }
    if let Some(section) = result.section.as_deref() {
        print_field(styler, "section", section);
    }
    if let Some(block_id) = result.block_id.as_deref() {
        print_field(styler, "block id", block_id);
    }
    if !result.needs.is_empty() {
        let needs = result
            .needs
            .iter()
            .map(|need| need.label())
            .collect::<Vec<_>>()
            .join(", ");
        print_field(styler, "needs", &needs);
    }

    if !result.sub_bullets.is_empty() {
        println!();
        println!("  Sub-bullets");
        for sub_bullet in &result.sub_bullets {
            println!("    - {sub_bullet}");
        }
    }

    if !result.spans.is_empty() {
        println!();
        println!("  Spans");
        for span in &result.spans {
            println!(
                "    {}  {}",
                styler.dim(&format!("{}-{}", span.start, span.end)),
                styler.cyan(span.kind.label())
            );
        }
    }

    if !result.diagnostics.is_empty() {
        println!();
        println!("  Diagnostics");
        for diagnostic in &result.diagnostics {
            let range = diagnostic
                .range
                .map(|(start, end)| format!("{start}-{end}"))
                .unwrap_or_else(|| "-".to_string());
            println!(
                "    {} {}  {}  {}",
                styled_severity(styler, diagnostic.severity),
                styler.dim(&range),
                styler.cyan(diagnostic.code),
                diagnostic.message
            );
        }
    }
}

fn print_field(styler: &Styler, label: &str, value: &str) {
    let rendered = if value.is_empty() {
        styler.dim("(empty)")
    } else {
        value.to_string()
    };
    println!("  {}  {rendered}", styler.dim(&pad_label(label)));
}

fn pad_label(label: &str) -> String {
    format!("{label:<8}")
}

fn styled_severity(styler: &Styler, severity: Severity) -> String {
    match severity {
        Severity::Error => styler.red(severity.label()),
        Severity::Warning => styler.yellow(severity.label()),
        Severity::Info => styler.blue(severity.label()),
    }
}

fn success_json(result: &CaptureParseResult) -> String {
    serde_json::to_string(result).expect("serialize capture parse result")
}

fn print_parse_error(message: &str, output_format: OutputFormat) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {message}"),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": message }))
        }
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> CaptureParseResult {
        CaptureParseResult::new(input.to_string())
    }

    fn json(input: &str) -> serde_json::Value {
        serde_json::from_str(&success_json(&parse(input))).expect("json")
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn cli_joins_text_arguments_with_spaces() {
        let matches = build_cli()
            .try_get_matches_from(vec![
                COMMAND_NAME,
                "--",
                "Call",
                "bank",
                "@Cash^",
            ])
            .expect("parse arguments");
        assert_eq!(
            raw_text_from_matches(&matches).expect("text"),
            "Call bank @Cash^"
        );
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Human);
    }

    #[test]
    fn cli_accepts_the_json_format_alias() {
        let matches = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "json", "body"])
            .expect("parse arguments");
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Json);
    }

    #[test]
    fn cli_rejects_an_unknown_format() {
        let error = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "yaml", "body"])
            .expect_err("unknown format");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn cli_keeps_hyphenated_text_literal_like_bob_capture() {
        let matches = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "--", "-x", "@dev^"])
            .expect("parse arguments");
        assert_eq!(raw_text_from_matches(&matches).expect("text"), "-x @dev^");
    }

    #[test]
    fn json_shape_is_stable() {
        let value = json("Call bank @Cash^");
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "Call bank @Cash^");
        assert_eq!(value["body"], "Call bank");
        assert_eq!(value["mode"], "incomplete");
        assert_eq!(value["route"], "cash");
        assert!(value["section"].is_null());
        assert!(value["block_id"].is_null());
        assert_eq!(value["needs"][0], "task");
        assert_eq!(value["spans"][0]["start"], 10);
        assert_eq!(value["spans"][0]["end"], 15);
        assert_eq!(value["spans"][0]["kind"], "sub_bullet_route");
        assert_eq!(value["spans"][1]["start"], 15);
        assert_eq!(value["spans"][1]["end"], 16);
        assert_eq!(value["spans"][1]["kind"], "interactive_placeholder");
        assert_eq!(value["diagnostics"].as_array().expect("array").len(), 0);
    }

    #[test]
    fn json_reports_every_mode_and_marker_kind() {
        assert_eq!(json("buy milk")["mode"], "task");
        assert_eq!(json("jot idea @notes#Ideas")["mode"], "bullet");
        assert_eq!(json("do work @dev:focus-1")["mode"], "pomodoro_task");
        assert_eq!(json("note @dev^focus-1")["mode"], "sub_bullet");
        assert_eq!(json("note @dev^")["mode"], "incomplete");

        let value = json("body p:2 s:1 % @groceries");
        assert_eq!(value["body"], "body");
        assert_eq!(
            value["spans"]
                .as_array()
                .expect("array")
                .iter()
                .map(|span| span["kind"].as_str().expect("kind"))
                .collect::<Vec<_>>(),
            vec!["priority", "schedule", "clipboard", "route"]
        );
    }

    #[test]
    fn json_reports_diagnostics_with_a_range_pair() {
        let value = json("note @dev^bad.id");
        assert_eq!(value["ok"], true);
        assert_eq!(value["mode"], "task");
        assert_eq!(
            value["diagnostics"][0]["code"],
            "invalid_sub_bullet_block_id"
        );
        assert_eq!(value["diagnostics"][0]["severity"], "error");
        assert_eq!(value["diagnostics"][0]["range"][0], 5);
        assert_eq!(value["diagnostics"][0]["range"][1], 16);
    }

    #[test]
    fn human_output_is_plain_without_color() {
        let styler = Styler::plain();
        assert!(!styler.is_color());
        // Rendering must not panic for a parse that exercises every branch.
        print_human_success_with_styler(
            &parse("note @dev^bad.id % s:1"),
            &styler,
        );
        print_human_success_with_styler(&parse("@"), &styler);
    }

    #[test]
    fn missing_text_uses_the_shared_capture_message() {
        assert_eq!(
            capture_language::missing_text_error(),
            "task text is required; pass TEXT or pipe it on stdin"
        );
    }

    #[test]
    fn spans_stay_ordered_and_on_character_boundaries() {
        let raw = "caf\u{e9} \u{1f680} @Cash^goog-exit s:2";
        let result = parse(raw);
        assert!(!result.spans.is_empty());
        for span in &result.spans {
            assert!(raw.is_char_boundary(span.start));
            assert!(raw.is_char_boundary(span.end));
        }
        for pair in result.spans.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
    }
}
