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
    capture_language::{self, DraftRewrite, RewriteRule, TextEdit},
    style::Styler,
};

const COMMAND_NAME: &str = "bob capture-rewrite";

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
    let cursor = matches.get_one::<usize>("cursor").copied();
    let raw_text = match raw_text_from_matches(&matches) {
        Ok(raw_text) => raw_text,
        Err(error) => return print_error(&error, output_format),
    };
    if capture_language::normalize_task_text(&raw_text).is_empty() {
        return print_error(&missing_text_error(), output_format);
    }
    if let Some(position) = cursor
        && (position > raw_text.len() || !raw_text.is_char_boundary(position))
    {
        return print_error(
            "--cursor must be a UTF-8 byte boundary within TEXT",
            output_format,
        );
    }

    let result = CaptureRewriteResult::new(raw_text, cursor);
    print_success(&result, output_format);
    0
}

fn missing_text_error() -> String {
    capture_language::missing_text_error()
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
        .about("Apply the capture grammar's automatic draft rewrites")
        .long_about(
            "Apply the capture grammar's editor typing assists to TEXT and \
report the resulting edits, cursor, and a human summary.\n\n\
The command is purely lexical and completely read-only: it never opens the \
vault, never reads the clipboard, never touches the filesystem, and takes no \
--bob-dir. Today it implements the bare '@@' absorption rule: typing a bare \
'@@' inside an item that already carries a local destination marker (or, \
absent that, when the draft already carries exactly one other '@@' \
declaration) rewrites the bare token to '@@<payload>', deletes the token it \
absorbed, and deletes every other declaration token in the draft, so the \
result always carries at most one declaration. Only the bare '@@' at \
--cursor (or, when --cursor is omitted, the last bare '@@' in source order) \
is a candidate. An item whose single local marker cannot be expressed as a \
declaration -- '@route#Section', '@route+block-id#section', \
'@route^block-id', '@route:block-id', or a trailing bare '#' -- is left \
untouched with a notice explaining why; an item \
with more than one local marker is left untouched with no notice, since \
'bob capture-parse' already reports that duplicate. Feeding a rewrite's own \
output back in is a no-op, because the claiming token is no longer bare.\n\n\
Only a missing TEXT or a bad flag is an error; every other input succeeds \
with 'changed: false' when nothing needed to change. If TEXT is omitted and \
stdin is piped, it reads the complete piped stdin stream.",
        )
        .after_help(
            "Examples:\n  bob capture-rewrite -c 16 -f json -- 'Buy milk @dev @@'\n  bob capture-rewrite -- 'Called Morgan Stanley @cash+goog-exit @@'\n  printf 'Buy milk @dev @@' | bob capture-rewrite -f json\n  printf '@@foo\\nBuy milk @@\\n' | bob capture-rewrite -f json\n  printf 'Buy milk @dev\\n- more detail @@\\n' | bob capture-rewrite -f json",
        )
        .disable_help_flag(true)
        .arg(cursor_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(text_arg())
}

fn cursor_arg() -> Arg {
    Arg::new("cursor")
        .long("cursor")
        .short('c')
        .value_name("N")
        .value_parser(clap::value_parser!(usize))
        .help("UTF-8 byte offset of the editor's insertion point")
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

/// Mirror `bob capture-parse`'s convention: join every TEXT argument with
/// spaces, or read the complete piped stdin stream when TEXT is omitted.
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
struct CaptureRewriteResult {
    ok: bool,
    schema_version: u32,
    input: String,
    text: String,
    changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<&'static str>,
    edits: Vec<EditJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct EditJson {
    range: SourceRange,
    replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct SourceRange {
    start: usize,
    end: usize,
}

impl CaptureRewriteResult {
    fn new(input: String, cursor: Option<usize>) -> Self {
        let rewrite = capture_language::rewrite_draft(&input, cursor);
        Self::from_rewrite(input, rewrite)
    }

    fn from_rewrite(input: String, rewrite: DraftRewrite) -> Self {
        let changed = rewrite.rule.is_some();
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            input,
            text: rewrite.text,
            changed,
            cursor: rewrite.cursor,
            rule: rewrite.rule.map(RewriteRule::code),
            edits: rewrite.edits.iter().map(edit_json).collect(),
            summary: rewrite.summary,
            notices: rewrite.notices,
        }
    }
}

fn edit_json(edit: &TextEdit) -> EditJson {
    EditJson {
        range: SourceRange {
            start: edit.start,
            end: edit.end,
        },
        replacement: edit.replacement.clone(),
    }
}

fn print_success(result: &CaptureRewriteResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureRewriteResult) {
    let styler = Styler::detect();
    print_human_success_with_styler(result, &styler);
}

fn print_human_success_with_styler(
    result: &CaptureRewriteResult,
    styler: &Styler,
) {
    if !result.changed {
        println!("{}", styler.dim("no rewrite"));
        for notice in &result.notices {
            println!("  {notice}");
        }
        return;
    }

    let rule = result.rule.unwrap_or("rewrite");
    println!(
        "Capture rewrite {} {}",
        styler.separator(),
        styler.cyan(rule)
    );
    println!();
    println!("  {}  {}", styler.dim("before"), result.input);
    println!("  {}  {}", styler.dim("after "), styler.green(&result.text));
    if let Some(summary) = &result.summary {
        println!();
        println!("  {summary}");
    }
    for notice in &result.notices {
        println!("  {notice}");
    }
}

fn success_json(result: &CaptureRewriteResult) -> String {
    serde_json::to_string(result).expect("serialize capture rewrite result")
}

fn print_error(message: &str, output_format: OutputFormat) -> i32 {
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

    fn rewrite(input: &str, cursor: Option<usize>) -> CaptureRewriteResult {
        CaptureRewriteResult::new(input.to_string(), cursor)
    }

    fn json(input: &str, cursor: Option<usize>) -> serde_json::Value {
        serde_json::from_str(&success_json(&rewrite(input, cursor)))
            .expect("json")
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
                "Buy",
                "milk",
                "@dev",
                "@@",
            ])
            .expect("parse arguments");
        assert_eq!(
            raw_text_from_matches(&matches).expect("text"),
            "Buy milk @dev @@"
        );
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Human);
    }

    #[test]
    fn cli_accepts_the_json_format_alias() {
        let matches = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "json", "@@"])
            .expect("parse arguments");
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Json);
    }

    #[test]
    fn cli_rejects_an_unknown_format() {
        let error = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "yaml", "@@"])
            .expect_err("unknown format");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn json_shape_absorbs_the_local_marker() {
        let value = json("Buy milk @dev @@", Some(16));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "Buy milk @dev @@");
        assert_eq!(value["text"], "Buy milk @@dev");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 14);
        assert_eq!(value["rule"], "absorb_local_marker");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 9, "end": 14 }, "replacement": "" },
                { "range": { "start": 14, "end": 16 }, "replacement": "@@dev" },
            ])
        );
        assert_eq!(value["summary"], "Moved @dev into @@dev");
        assert!(value.get("notices").is_none(), "{value}");
    }

    #[test]
    fn json_omits_cursor_when_not_supplied() {
        let value = json("Buy milk @dev @@", None);
        assert_eq!(value["changed"], true);
        assert!(value.get("cursor").is_none(), "{value}");
    }

    #[test]
    fn json_reports_no_rewrite_with_no_bare_at_at() {
        let value = json("Buy milk @dev", None);
        assert_eq!(value["ok"], true);
        assert_eq!(value["changed"], false);
        assert_eq!(value["text"], "Buy milk @dev");
        assert!(value.get("rule").is_none(), "{value}");
        assert!(value.get("summary").is_none(), "{value}");
    }

    #[test]
    fn json_reports_a_rule_a5_notice_without_changing_text() {
        let value = json("note @notes#Ideas @@", None);
        assert_eq!(value["changed"], false);
        assert_eq!(value["text"], "note @notes#Ideas @@");
        assert_eq!(value["notices"].as_array().expect("notices").len(), 1);
        assert!(value["notices"][0]
            .as_str()
            .expect("notice")
            .contains("cannot take a section"));
    }

    #[test]
    fn human_output_is_plain_without_color() {
        let styler = Styler::plain();
        assert!(!styler.is_color());
        print_human_success_with_styler(
            &rewrite("Buy milk @dev @@", Some(16)),
            &styler,
        );
        print_human_success_with_styler(
            &rewrite("Buy milk @dev", None),
            &styler,
        );
    }

    #[test]
    fn missing_text_reports_a_usage_error() {
        assert_eq!(
            missing_text_error(),
            "task text is required; pass TEXT or pipe it on stdin"
        );
    }
}
