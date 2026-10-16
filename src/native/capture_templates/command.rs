use std::{
    ffi::OsString,
    io::{self, IsTerminal, Read},
    iter,
    path::Path,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::store::{self, TemplateShelfEntry, TemplateShelfSnapshot};
use crate::native::{
    capture_language,
    capture_parse::{self, CaptureParseSpan},
    config,
    style::Styler,
};

const COMMAND_NAME: &str = "bob capture-templates";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    List,
    Show,
    Save,
    Delete,
    Clear,
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

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    run(Command::List, args)
}

pub(crate) fn run_show(args: Vec<OsString>) -> i32 {
    run(Command::Show, args)
}

pub(crate) fn run_save(args: Vec<OsString>) -> i32 {
    run(Command::Save, args)
}

pub(crate) fn run_delete(args: Vec<OsString>) -> i32 {
    run(Command::Delete, args)
}

pub(crate) fn run_clear(args: Vec<OsString>) -> i32 {
    run(Command::Clear, args)
}

pub(crate) fn build_cli_list() -> ClapCommand {
    build_cli(Command::List)
}

pub(crate) fn build_cli_show() -> ClapCommand {
    build_cli(Command::Show)
}

pub(crate) fn build_cli_save() -> ClapCommand {
    build_cli(Command::Save)
}

pub(crate) fn build_cli_delete() -> ClapCommand {
    build_cli(Command::Delete)
}

pub(crate) fn build_cli_clear() -> ClapCommand {
    build_cli(Command::Clear)
}

fn run(command_kind: Command, args: Vec<OsString>) -> i32 {
    let mut command = build_cli(command_kind);
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(command_name(command_kind))).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);

    match command_kind {
        Command::List => run_list_command(format),
        Command::Show => {
            let selector = matches
                .get_one::<String>("template")
                .map(String::as_str)
                .unwrap_or_default();
            run_show_command(selector, format)
        }
        Command::Save => run_save_command(&matches, format),
        Command::Delete => {
            let selectors = matches
                .get_many::<String>("template")
                .map(|values| values.cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            run_delete_command(&selectors, format)
        }
        Command::Clear => run_clear_command(format),
    }
}

fn build_cli(kind: Command) -> ClapCommand {
    let mut command = ClapCommand::new(command_name(kind))
        .disable_help_flag(true)
        .arg(format_arg())
        .arg(help_arg());
    command = match kind {
        Command::List => command
            .about("List saved capture templates")
            .long_about(
                "List the bob-owned saved `@@template:` blocks. The command is read-only. Bare `bob capture-templates` runs this list.",
            )
            .after_help(
                "Examples:\n  bob capture-templates\n  bob capture-templates list -f json\n  bob capture-templates show 1",
            ),
        Command::Show => command
            .about("Print one template as a ready-to-insert draft")
            .arg(template_arg())
            .long_about(
                "Print the selected template as an unstyled `@@template:` draft. TEMPLATE is a 1-based list index (one or two digits) or a content id.",
            )
            .after_help(
                "Examples:\n  bob capture-templates show 1\n  bob capture-templates show 9c1f0e2a7b3d -f json",
            ),
        Command::Save => command
            .about("Save a template definition without capturing a row")
            .arg(template_origin_arg())
            .arg(text_arg())
            .long_about(
                "Save one template definition from TEXT or stdin. Only definition errors are checked; missing rows and row errors are ignored. A successful save does not increment use_count. `--template-origin` is an internal checkout id used by Bob Mac Capture.",
            )
            .after_help(
                "Examples:\n  bob capture-templates save '@@template:\\nApply to {0} @job^apply-{1}\\n\\n'\n  cat template-draft.txt | bob capture-templates save -f json",
            ),
        Command::Delete => command
            .about("Delete templates by list index or content id")
            .arg(templates_arg())
            .long_about(
                "Delete one or more entries. TEMPLATE is a 1-based list index (one or two digits) or a content id. Deleting a missing entry succeeds.",
            )
            .after_help(
                "Examples:\n  bob capture-templates delete 1\n  bob capture-templates delete 1 9c1f0e2a7b3d -f json",
            ),
        Command::Clear => command
            .about("Clear the saved-template shelf")
            .long_about(
                "Remove the shelf and its corrupt-file quarantine copy. This is also the recovery path when a newer bob has written the shelf schema.",
            )
            .after_help("Example:\n  bob capture-templates clear -f json"),
    };
    command
}

fn command_name(kind: Command) -> &'static str {
    match kind {
        Command::List => "bob capture-templates list",
        Command::Show => "bob capture-templates show",
        Command::Save => "bob capture-templates save",
        Command::Delete => "bob capture-templates delete",
        Command::Clear => "bob capture-templates clear",
    }
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

fn template_arg() -> Arg {
    Arg::new("template")
        .value_name("TEMPLATE")
        .required(true)
        .value_parser(clap::builder::ValueParser::string())
        .help("1-based list index or content id")
}

fn templates_arg() -> Arg {
    Arg::new("template")
        .value_name("TEMPLATE")
        .num_args(1..)
        .action(ArgAction::Append)
        .value_parser(clap::builder::ValueParser::string())
        .help("1-based list index or content id")
}

fn template_origin_arg() -> Arg {
    Arg::new("template-origin")
        .long("template-origin")
        .value_name("ID")
        .hide(true)
        .help("Internal saved-template checkout id")
}

fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(OsStringValueParser::new())
        .help("Template draft; multiple args are joined with spaces")
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

fn run_list_command(format: OutputFormat) -> i32 {
    let (capacity, mut warnings) = configured_capacity();
    let snapshot = store::read_shelf();
    warnings.extend(snapshot.warnings.clone());
    emit_warnings(&warnings);
    let templates = snapshot
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| template_json(index + 1, entry))
        .collect::<Vec<_>>();

    match format {
        OutputFormat::Json => println!(
            "{}",
            json!({
                "ok": true,
                "schema_version": SCHEMA_VERSION,
                "capacity": capacity,
                "saving": capacity > 0,
                "file": store::templates_file().to_string_lossy(),
                "templates": templates,
                "warnings": warnings,
            })
        ),
        OutputFormat::Human => {
            print_human_list(&snapshot, &templates, capacity)
        }
    }
    0
}

fn run_show_command(selector: &str, format: OutputFormat) -> i32 {
    let snapshot = store::read_shelf();
    emit_warnings(&snapshot.warnings);
    let Some((index, entry)) = resolve_selector(&snapshot.entries, selector)
    else {
        return report_error(
            format,
            "saved_template_not_found",
            &format!("saved template {selector:?} was not found"),
            1,
        );
    };
    let template = template_json(index + 1, entry);
    match format {
        OutputFormat::Json => {
            println!("{}", json!({"ok": true, "template": template}))
        }
        OutputFormat::Human => {
            print!("{}", template.draft);
        }
    }
    0
}

fn run_save_command(matches: &ArgMatches, format: OutputFormat) -> i32 {
    if let Some(origin) = matches.get_one::<String>("template-origin")
        && !store::is_valid_origin(origin)
    {
        return report_usage_error(
            "save",
            "--template-origin must be 1-64 ASCII alphanumeric characters",
        );
    }

    let raw = match raw_text_from_matches(matches) {
        Ok(raw) => raw,
        Err(message) => {
            return report_error(format, "saved_templates_io", &message, 1);
        }
    };
    let analysis = capture_language::template::analyze(&raw);
    if analysis.header.is_none()
        && !analysis
            .issues
            .iter()
            .any(|issue| issue.code() == "template_header_misplaced")
    {
        return report_error(
            format,
            "saved_template_not_template",
            "start the draft with `@@template:` to save it as a template",
            2,
        );
    }
    if let Some(issue) = capture_language::template::definition_issue(&raw) {
        return report_error(format, issue.code(), &issue.message(), 2);
    }
    let Some(canonical) = store::canonical_template_text(&raw) else {
        return report_error(
            format,
            "saved_template_not_template",
            "start the draft with `@@template:` to save it as a template",
            2,
        );
    };

    let (capacity, warnings) = configured_capacity();
    emit_warnings(&warnings);
    if capacity == 0 {
        return report_error(
            format,
            "saved_templates_disabled",
            "saving templates is off (capture.max_saved_templates: 0)",
            2,
        );
    }
    let now = store::now_timestamp();
    let origin = matches
        .get_one::<String>("template-origin")
        .map(String::as_str);
    let saved = match store::save_template_definition(
        &canonical, origin, capacity, &now,
    ) {
        Ok(saved) => saved,
        Err(error) => return report_save_error(format, error),
    };
    emit_warnings(&saved.warnings);
    let snapshot = store::read_shelf();
    let Some((index, entry)) = snapshot
        .entries
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.id == saved.id)
    else {
        return report_error(
            format,
            "saved_templates_io",
            "saved template could not be read back from the shelf",
            1,
        );
    };
    let template = template_json(index + 1, entry);
    let status = saved.status.as_str();
    match format {
        OutputFormat::Json => println!(
            "{}",
            json!({
                "ok": true,
                "saved": { "id": saved.id, "status": status },
                "template": template,
            })
        ),
        OutputFormat::Human => {
            let route = template
                .route
                .as_deref()
                .map(|route| format!(" · {route}"))
                .unwrap_or_default();
            println!(
                "✓ saved template {} · {} field{}{}",
                saved.id,
                template.field_count,
                if template.field_count == 1 { "" } else { "s" },
                route
            );
        }
    }
    0
}

fn run_delete_command(selectors: &[String], format: OutputFormat) -> i32 {
    let deleted = match store::delete_templates(selectors) {
        Ok(outcome) => {
            emit_warnings(&outcome.warnings);
            outcome
        }
        Err(error) => return report_store_error(format, error),
    };
    match format {
        OutputFormat::Json => println!(
            "{}",
            json!({
                "ok": true,
                "deleted": deleted.deleted,
                "missing": deleted.missing,
            })
        ),
        OutputFormat::Human => println!(
            "✓ deleted {} template{}",
            deleted.deleted.len(),
            if deleted.deleted.len() == 1 { "" } else { "s" }
        ),
    }
    0
}

fn run_clear_command(format: OutputFormat) -> i32 {
    let cleared = match store::clear_templates() {
        Ok(outcome) => {
            emit_warnings(&outcome.warnings);
            outcome
        }
        Err(error) => return report_store_error(format, error),
    };
    match format {
        OutputFormat::Json => {
            println!("{}", json!({"ok": true, "cleared": cleared.cleared}))
        }
        OutputFormat::Human => println!(
            "✓ cleared {} template{}",
            cleared.cleared,
            if cleared.cleared == 1 { "" } else { "s" }
        ),
    }
    0
}

fn configured_capacity() -> (u32, Vec<String>) {
    match config::load_capture_config(&config::config_path()) {
        Ok(config) => (config.max_saved_templates(), Vec::new()),
        Err(error) => {
            let message = match error {
                config::ConfigError::Read(message)
                | config::ConfigError::Invalid(message) => message,
            };
            (
                config::DEFAULT_MAX_SAVED_TEMPLATES,
                vec![format!("{message}; using 36")],
            )
        }
    }
}

fn emit_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("{COMMAND_NAME}: warning: {warning}");
    }
}

fn report_store_error(
    format: OutputFormat,
    error: store::TemplateShelfError,
) -> i32 {
    let (code, exit) = match error {
        store::TemplateShelfError::Busy(_) => ("saved_templates_busy", 1),
        store::TemplateShelfError::NewerSchema(_) => {
            ("saved_templates_newer_schema", 1)
        }
        store::TemplateShelfError::Io(_) => ("saved_templates_io", 1),
    };
    report_error(format, code, &error.message(), exit)
}

fn report_save_error(
    format: OutputFormat,
    error: store::TemplateSaveError,
) -> i32 {
    let (code, exit) = match &error {
        store::TemplateSaveError::Busy(_) => ("saved_templates_busy", 1),
        store::TemplateSaveError::Refused(_) => {
            ("saved_templates_newer_schema", 1)
        }
        store::TemplateSaveError::Io(_) => ("saved_templates_io", 1),
    };
    report_error(format, code, error.reason(), exit)
}

fn report_error(
    format: OutputFormat,
    code: &str,
    message: &str,
    exit: i32,
) -> i32 {
    match format {
        OutputFormat::Json => {
            println!("{}", json!({"ok": false, "error": message, "code": code}))
        }
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {message}"),
    }
    exit
}

fn report_usage_error(verb: &str, message: &str) -> i32 {
    eprintln!("{COMMAND_NAME} {verb}: error: {message}");
    2
}

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

fn resolve_selector<'a>(
    entries: &'a [TemplateShelfEntry],
    selector: &str,
) -> Option<(usize, &'a TemplateShelfEntry)> {
    let index = if is_short_numeric_selector(selector) {
        selector.parse::<usize>().ok()?.checked_sub(1)?
    } else {
        entries.iter().position(|entry| entry.id == selector)?
    };
    entries.get(index).map(|entry| (index, entry))
}

fn is_short_numeric_selector(selector: &str) -> bool {
    !selector.is_empty()
        && selector.len() <= 2
        && selector.bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Debug, Clone, Serialize)]
struct TemplateJson {
    index: usize,
    id: String,
    text: String,
    draft: String,
    cursor: usize,
    field_count: usize,
    line_count: usize,
    route: Option<String>,
    spans: Vec<CaptureParseSpan>,
    first_saved_at: Option<String>,
    last_used_at: Option<String>,
    use_count: u64,
}

fn template_json(index: usize, entry: &TemplateShelfEntry) -> TemplateJson {
    let draft = format!("@@template:\n{}\n\n", entry.text);
    let parsed = capture_parse::parse_template_protocol(&entry.text);
    let analysis = capture_language::template::analyze(&draft);
    TemplateJson {
        index,
        id: entry.id.clone(),
        text: entry.text.clone(),
        cursor: draft.len(),
        draft,
        field_count: analysis.field_count,
        line_count: if entry.text.is_empty() {
            0
        } else {
            entry.text.split('\n').count()
        },
        route: parsed.route,
        spans: parsed.spans,
        first_saved_at: entry.first_saved_at.clone(),
        last_used_at: entry.last_used_at.clone(),
        use_count: entry.use_count,
    }
}

fn print_human_list(
    snapshot: &TemplateShelfSnapshot,
    templates: &[TemplateJson],
    capacity: u32,
) {
    let styler = Styler::detect();
    let file = display_path(&store::templates_file());
    let mut heading = format!(
        "Saved templates {} {} of {} {} {file}",
        styler.separator(),
        snapshot.entries.len(),
        capacity,
        styler.separator()
    );
    if capacity == 0 {
        heading.push_str("  saving is off (capture.max_saved_templates: 0)");
    }
    println!("{heading}");
    if snapshot.entries.is_empty() {
        println!();
        println!("No saved templates yet. Capture a `@@template:` draft and it lands here.");
        return;
    }

    for (entry, template) in snapshot.entries.iter().zip(templates) {
        println!();
        let text_lines = entry.text.split('\n').collect::<Vec<_>>();
        let mut byte_start = 0;
        for (line_index, line) in text_lines.iter().enumerate() {
            let colored =
                paint_template_line(line, byte_start, &template.spans, &styler);
            if line_index == 0 {
                println!("{:>2}  {colored}", template.index);
            } else {
                println!("    {colored}");
            }
            byte_start += line.len() + 1;
        }
        let mut metadata = format!(
            "{} field{}",
            template.field_count,
            if template.field_count == 1 { "" } else { "s" }
        );
        if let Some(route) = template.route.as_deref() {
            metadata.push_str(" · ");
            metadata.push_str(&styler.cyan(route));
        }
        if template.use_count == 0 {
            if let Some(age) = relative_time(
                template.first_saved_at.as_deref(),
                &store::now_timestamp(),
            ) {
                metadata.push_str(" · saved ");
                metadata.push_str(&age);
            }
        } else {
            metadata.push_str(&format!(" · used {}×", template.use_count));
            if let Some(age) = relative_time(
                template.last_used_at.as_deref(),
                &store::now_timestamp(),
            ) {
                metadata.push_str(" · ");
                metadata.push_str(&age);
            }
        }
        metadata.push_str(" · ");
        metadata.push_str(&styler.dim(&template.id));
        println!("    {}", styler.dim(&metadata));
    }
}

fn paint_template_line(
    line: &str,
    line_start: usize,
    spans: &[CaptureParseSpan],
    styler: &Styler,
) -> String {
    let line_end = line_start + line.len();
    let mut result = String::new();
    let mut cursor = line_start;
    for span in spans.iter().filter(|span| {
        span.start < line_end
            && span.end > line_start
            && (span.kind.label() == "template_placeholder"
                || is_route_span(span.kind.label()))
    }) {
        let start = span.start.max(line_start);
        let end = span.end.min(line_end);
        if start < cursor || end > line_end || start > end {
            continue;
        }
        let local_cursor = cursor - line_start;
        let local_start = start - line_start;
        result.push_str(&line[local_cursor..local_start]);
        let token = &line[local_start..(end - line_start)];
        let painted = if span.kind.label() == "template_placeholder" {
            slot_color(span.field.unwrap_or(0), token, styler)
        } else {
            styler.cyan(token)
        };
        result.push_str(&painted);
        cursor = end;
    }
    result.push_str(&line[(cursor - line_start)..]);
    result
}

fn is_route_span(kind: &str) -> bool {
    matches!(
        kind,
        "route"
            | "task_block_id_route"
            | "pomodoro_route"
            | "active_task_route"
            | "sub_bullet_route"
            | "task_toggle_route"
    )
}

fn slot_color(field: usize, text: &str, styler: &Styler) -> String {
    const COLORS: [&str; 6] = [
        "38;5;208", "38;5;37", "38;5;211", "38;5;40", "38;5;141", "38;5;130",
    ];
    styler.paint(COLORS[field % COLORS.len()], text)
}

fn display_path(path: &Path) -> String {
    let path_text = path.to_string_lossy();
    if let Some(home) = crate::native::env::home_dir().to_str()
        && let Some(suffix) = path_text.strip_prefix(home)
        && (suffix.is_empty() || suffix.starts_with(std::path::MAIN_SEPARATOR))
    {
        return format!("~{suffix}");
    }
    path_text.into_owned()
}

fn relative_time(timestamp: Option<&str>, now: &str) -> Option<String> {
    let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp?).ok()?;
    let now = chrono::DateTime::parse_from_rfc3339(now).ok()?;
    let seconds = now.signed_duration_since(timestamp).num_seconds();
    if seconds < 0 {
        return Some(format!("in {}", age_unit(seconds.unsigned_abs())));
    }
    Some(format!("{} ago", age_unit(seconds as u64)))
}

fn age_unit(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        86_400..=604_799 => format!("{}d", seconds / 86_400),
        604_800..=2_419_199 => format!("{}w", seconds / 604_800),
        2_419_200..=31_535_999 => format!("{}mo", seconds / 2_419_200),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_use_one_or_two_digits_for_one_based_indices() {
        assert!(is_short_numeric_selector("1"));
        assert!(is_short_numeric_selector("36"));
        assert!(!is_short_numeric_selector("123"));
        assert!(!is_short_numeric_selector("9c1f0e2a7b3d"));
    }

    #[test]
    fn relative_time_uses_the_bob_clock_value() {
        assert_eq!(
            relative_time(
                Some("2026-10-16T09:38:02+00:00"),
                "2026-10-16T09:40:02+00:00"
            )
            .as_deref(),
            Some("2m ago")
        );
    }
}
