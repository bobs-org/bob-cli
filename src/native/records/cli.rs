//! Public `bob records` commands.

use std::{
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand, ValueHint,
};
use serde_json::{json, Value};

use super::{
    catalog::RecordCatalog,
    check::TypeReport,
    plan::{plan_create, RecordCreateRequest},
    render::RecordNoteInput,
    schema::{
        FieldInputState, FieldKind, RecordDate, RecordFieldInput, RecordScalar,
        RecordValue, RecordsConfig,
    },
};
use crate::native::{
    capture::{write_staged_files, StagedTextFile},
    config::{self, records::load_records_config},
    env as bob_env,
    style::Styler,
};

const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run_check(args: Vec<OsString>) -> i32 {
    run_parsed(args, build_cli_check(), execute_check)
}

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    run_parsed(args, build_cli_list(), execute_list)
}

pub(crate) fn run_new(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_new();
    let matches = match matches_for(&mut command, "bob records new", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    execute_new(&matches)
}

pub(crate) fn run_show(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_show();
    let matches = match matches_for(&mut command, "bob records show", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    let output_format = format_from(&matches);
    let vault = vault_from(&matches);
    match load_config() {
        Ok(config) => {
            let Some(record_type) = matches
                .get_one::<String>("type")
                .and_then(|id| config.record_type(id))
            else {
                let id = matches
                    .get_one::<String>("type")
                    .map_or("", String::as_str);
                return report_error(
                    "bob records show",
                    output_format,
                    &format!("unknown record type `{id}`"),
                    2,
                );
            };
            let mut catalog = RecordCatalog::new(vault);
            let folder = catalog.folder(&record_type.dir);
            let fields: Vec<Value> = record_type
                .fields
                .iter()
                .map(|field| {
                    let kind = kind_name(&field.kind);
                    let choices: Vec<Value> = match &field.kind {
                        FieldKind::Choices(choices) => choices
                            .iter()
                            .map(|choice| json!({
                                "value": choice.value,
                                "label": choice.label,
                            }))
                            .collect(),
                        _ => Vec::new(),
                    };
                    let top_values: Vec<Value> = if field.suggest {
                        folder
                            .value_suggestions(&field.key)
                            .into_iter()
                            .take(5)
                            .map(|value| json!({
                                "value": value.insert,
                                "count": value.count,
                            }))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    json!({
                        "key": field.key,
                        "label": field.label,
                        "kind": kind,
                        "required": field.required,
                        "list": field.list,
                        "default": field.default.as_ref().map(|value| value_display(value)),
                        "choices": choices,
                        "top_values": top_values,
                    })
                })
                .collect();
            let payload = json!({
                "schema_version": SCHEMA_VERSION,
                "type": record_type.id,
                "label": record_type.label,
                "dir": record_type.dir,
                "emoji": record_type.emoji,
                "dated_notes": record_type.dated_notes,
                "count": folder.notes().len(),
                "fields": fields,
            });
            print_show(&payload, record_type, output_format);
            0
        }
        Err(error) => {
            report_config_error("bob records show", output_format, error)
        }
    }
}

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob records check")
        .about("Check configured Record Notes against their schemas")
        .long_about(
            "Lint each configured Record Note folder. Checks the type link, required and typed fields, legacy keys, and records filed outside their configured folder.",
        )
        .after_help(
            "Examples:\n  bob records check\n  bob records check restaurant\n  bob records check -f json\n\nSee docs/records.md for the schema and lint rules.",
        )
        .disable_help_flag(true)
        .arg(record_type_arg(false))
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_list() -> ClapCommand {
    ClapCommand::new("bob records list")
        .about("List configured Record Note collections (default)")
        .long_about(
            "Show each configured record type, its folder, note count, and required fields. This command is read-only. Bare `bob records` runs this command.",
        )
        .after_help(
            "Examples:\n  bob records\n  bob records list\n  bob records list -f json\n\nSee docs/records.md for record schemas.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_new() -> ClapCommand {
    ClapCommand::new("bob records new")
        .about("Create one Record Note from its configured schema")
        .long_about(
            "Plan and create a single Record Note. Supply fields with repeated --set KEY=VALUE arguments and notes with repeated --note arguments. Missing required fields are refused with a ready-to-copy draft. --dry-run prints the exact Markdown without writing.",
        )
        .after_help(
            "Examples:\n  bob records new restaurant 'Ramen Nagi' -s status=to_try -s category=Ramen -s location=Clark\n  bob records new podcast 'The Daily' -s status=to_try -s category=News -n 'Recommended by Kelly' -d\n  bob records new restaurant '1950 Originals' -S 1950s_originals -s status=to_try -s category=Pizza -s location=Westwood\n  bob records new restaurant 'Ninja Sushi' -d -f json\n\nSee docs/records.md for field kinds, defaults, and failure behavior.",
        )
        .disable_help_flag(true)
        .arg(record_type_arg(true))
        .arg(
            Arg::new("title")
                .value_name("TITLE")
                .required(true)
                .value_hint(ValueHint::Other)
                .help("Title of the new record note"),
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(
            Arg::new("note")
                .short('n')
                .long("note")
                .value_name("TEXT")
                .action(ArgAction::Append)
                .help("Append a note bullet; may be repeated"),
        )
        .arg(
            Arg::new("open")
                .short('o')
                .long("open")
                .action(ArgAction::SetTrue)
                .conflicts_with("dry-run")
                .help("Open the created note in Obsidian"),
        )
        .arg(
            Arg::new("set")
                .short('s')
                .long("set")
                .value_name("KEY=VALUE")
                .action(ArgAction::Append)
                .help("Set a configured field; may be repeated"),
        )
        .arg(
            Arg::new("stem")
                .short('S')
                .long("stem")
                .value_name("STEM")
                .help("Use an explicit filename stem"),
        )
}

pub(crate) fn build_cli_show() -> ClapCommand {
    ClapCommand::new("bob records show")
        .about("Show one record schema and its common values")
        .long_about(
            "Display a record type's ordered fields, kinds, required marks, defaults, choices, and most-used suggested values.",
        )
        .after_help(
            "Examples:\n  bob records show restaurant\n  bob records show podcast -f json\n\nSee docs/records.md for the schema contract.",
        )
        .disable_help_flag(true)
        .arg(record_type_arg(true))
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

fn run_parsed(
    args: Vec<OsString>,
    mut command: ClapCommand,
    execute: fn(&ArgMatches) -> i32,
) -> i32 {
    let name = command.get_name().to_owned();
    match matches_for(&mut command, &name, args) {
        Ok(matches) => execute(&matches),
        Err(code) => code,
    }
}

fn matches_for(
    command: &mut ClapCommand,
    name: &str,
    args: Vec<OsString>,
) -> Result<ArgMatches, i32> {
    match command
        .try_get_matches_from_mut(iter::once(OsString::from(name)).chain(args))
    {
        Ok(matches) => Ok(matches),
        Err(error) => Err(print_clap_error(error)),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "bob records: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn record_type_arg(required: bool) -> Arg {
    Arg::new("type")
        .value_name("TYPE")
        .required(required)
        .value_hint(ValueHint::Other)
        .help("Configured record type id")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .short('b')
        .long("bob-dir")
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .short('d')
        .long("dry-run")
        .action(ArgAction::SetTrue)
        .help("Print the planned note without writing")
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

fn help_arg() -> Arg {
    Arg::new("help")
        .short('h')
        .long("help")
        .action(ArgAction::Help)
        .help("Show help")
}

fn format_from(matches: &ArgMatches) -> OutputFormat {
    match matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("human")
    {
        "json" => OutputFormat::Json,
        _ => OutputFormat::Human,
    }
}

fn vault_from(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .unwrap_or_else(bob_env::bob_dir)
}

fn load_config() -> Result<RecordsConfig, super::schema::RecordDiagnostic> {
    load_records_config()
}

fn execute_list(matches: &ArgMatches) -> i32 {
    let output_format = format_from(matches);
    match load_config() {
        Ok(config) => {
            let mut catalog = RecordCatalog::new(vault_from(matches));
            let rows: Vec<Value> = config
                .types
                .iter()
                .map(|record_type| {
                    let folder = catalog.folder(&record_type.dir);
                    let required: Vec<&str> = record_type
                        .fields
                        .iter()
                        .filter(|field| field.required)
                        .map(|field| field.key.as_str())
                        .collect();
                    json!({
                        "id": record_type.id,
                        "label": record_type.label,
                        "dir": record_type.dir,
                        "emoji": record_type.emoji,
                        "count": folder.notes().len(),
                        "required_fields": required,
                    })
                })
                .collect();
            let payload = json!({
                "schema_version": SCHEMA_VERSION,
                "types": rows,
            });
            print_list(&payload, &config, output_format);
            0
        }
        Err(error) => {
            report_config_error("bob records list", output_format, error)
        }
    }
}

fn execute_check(matches: &ArgMatches) -> i32 {
    let output_format = format_from(matches);
    match load_config() {
        Ok(config) => {
            let selected_type =
                matches.get_one::<String>("type").map(String::as_str);
            if let Some(id) = selected_type
                && config.record_type(id).is_none()
            {
                return report_error(
                    "bob records check",
                    output_format,
                    &format!("unknown record type `{id}`"),
                    2,
                );
            }
            let vault = vault_from(matches);
            let today = bob_env::current_datetime().date();
            let mut catalog = RecordCatalog::new(vault.clone());
            let mut reports = Vec::new();
            for record_type in &config.types {
                if selected_type
                    .is_some_and(|selected| selected != record_type.id)
                {
                    continue;
                }
                let folder = catalog.folder(&record_type.dir);
                let mut report = TypeReport::new(record_type, folder);
                for note in folder.notes() {
                    report.check_note(record_type, note, today);
                }
                report.check_suggested_values(record_type, folder);
                for (path, note_kind) in [
                    (PathBuf::from(format!("{}.md", record_type.id)), "type"),
                    (
                        PathBuf::from(format!("{}.md", record_type.dir)),
                        "collection",
                    ),
                ] {
                    if note_kind == "collection"
                        && record_type.id == record_type.dir
                    {
                        continue;
                    }
                    if !vault.join(&path).is_file() {
                        let path_text = path.display().to_string();
                        report.push(
                            "error",
                            "record_missing_collection_note",
                            Some(&path_text),
                            None,
                            format!(
                                "required {note_kind} note is missing: {}",
                                path.display()
                            ),
                        );
                    }
                }
                reports.push(report);
            }
            add_external_records(&vault, &config, &mut reports);
            let errors: usize =
                reports.iter().map(|report| report.errors).sum();
            let warnings: usize =
                reports.iter().map(|report| report.warnings).sum();
            let payload = json!({
                "schema_version": SCHEMA_VERSION,
                "types": reports,
                "summary": { "errors": errors, "warnings": warnings },
            });
            print_check(&payload, &config, output_format);
            if errors > 0 {
                1
            } else {
                0
            }
        }
        Err(error) => {
            report_config_error("bob records check", output_format, error)
        }
    }
}

fn execute_new(matches: &ArgMatches) -> i32 {
    let output_format = format_from(matches);
    let vault = vault_from(matches);
    let type_id = matches.get_one::<String>("type").expect("required by clap");
    let title = matches
        .get_one::<String>("title")
        .expect("required by clap");
    let config = match load_config() {
        Ok(config) => config,
        Err(error) => {
            return report_config_error("bob records new", output_format, error)
        }
    };
    let timezone = match crate::native::day_timezone() {
        Ok(timezone) => timezone,
        Err(error) => {
            return report_error(
                "bob records new",
                output_format,
                &error.to_string(),
                2,
            )
        }
    };
    let fields = match parse_field_inputs(matches) {
        Ok(fields) => fields,
        Err(message) => {
            return report_error("bob records new", output_format, &message, 2)
        }
    };
    let notes: Vec<RecordNoteInput> = matches
        .get_many::<String>("note")
        .into_iter()
        .flatten()
        .map(|text| RecordNoteInput::bullet(text.clone()))
        .collect();
    let stem = matches.get_one::<String>("stem").map(String::as_str);
    let now = bob_env::current_datetime();
    let mut catalog = RecordCatalog::new(&vault);
    let plan = match plan_create(
        RecordCreateRequest {
            config: &config,
            record_type: type_id,
            title,
            explicit_stem: stem,
            fields: &fields,
            notes: &notes,
            now,
            timezone,
            vault_root: &vault,
            reserved_paths: &[],
        },
        &mut catalog,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return report_error(
                "bob records new",
                output_format,
                &error.to_string(),
                2,
            )
        }
    };

    let Some(markdown) = plan.markdown.as_deref() else {
        return report_incomplete(&config, &plan, output_format);
    };
    let target = vault.join(&plan.path);
    if matches.get_flag("dry-run") {
        print_new_dry_run(&plan, markdown, output_format);
        return 0;
    }

    if let Some(parent) = target.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return report_error(
            "bob records new",
            output_format,
            &format!(
                "could not create record folder {}: {error}",
                parent.display()
            ),
            2,
        );
    }
    let staged = StagedTextFile {
        target: target.clone(),
        target_existed: false,
        original_target: String::new(),
        updated_target: markdown.to_owned(),
    };
    if let Err(error) = write_staged_files(&[staged]) {
        return report_error(
            "bob records new",
            output_format,
            &error.message,
            2,
        );
    }

    let payload = json!({
        "schema_version": SCHEMA_VERSION,
        "status": "written",
        "type": plan.record_type,
        "title": plan.title,
        "path": plan.path.display().to_string(),
        "properties": plan.properties.iter().map(|property| json!({
            "key": property.key,
            "label": property.label,
            "state": property.state.as_str(),
            "value": property.display,
            "source": match property.source {
                Some(super::plan::RecordPropertySource::Typed) => Some("typed"),
                Some(super::plan::RecordPropertySource::Default) => Some("default"),
                None => None,
            },
        })).collect::<Vec<_>>(),
    });
    let record_type =
        config.record_type(type_id).expect("validated by planner");
    print_new_success(
        &payload,
        &plan,
        record_type.emoji.as_deref(),
        output_format,
    );
    if matches.get_flag("open")
        && let Err(message) = open_in_obsidian(&vault, &plan.path)
    {
        eprintln!("bob records new: warning: {message}");
    }
    0
}

fn parse_field_inputs(
    matches: &ArgMatches,
) -> Result<Vec<RecordFieldInput>, String> {
    let mut fields = Vec::new();
    for raw in matches.get_many::<String>("set").into_iter().flatten() {
        let Some((key, value)) = raw.split_once('=') else {
            return Err(format!(
                "invalid --set value {raw:?}: expected KEY=VALUE"
            ));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(
                "--set needs a nonempty field key before `=`".to_owned()
            );
        }
        fields.push(RecordFieldInput {
            key: key.to_owned(),
            state: if value.is_empty() {
                FieldInputState::SuppliedEmpty
            } else {
                FieldInputState::SuppliedValues(vec![value.to_owned()])
            },
            source_range: None,
        });
    }
    Ok(fields)
}

fn report_incomplete(
    config: &RecordsConfig,
    plan: &super::plan::RecordCreatePlan,
    format: OutputFormat,
) -> i32 {
    let record_type = config.record_type(&plan.record_type);
    let mut draft = format!(
        "{} @{}/",
        plan.title,
        record_type.map_or("", |kind| kind.dir.as_str())
    );
    if let Some(record_type) = record_type {
        for field in &record_type.fields {
            if !plan.missing.contains(&field.key) {
                continue;
            }
            let choices = match &field.kind {
                FieldKind::Choices(values) => values
                    .iter()
                    .map(|choice| choice.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" · "),
                _ => String::new(),
            };
            draft.push_str(&format!("\n- {}:: …", field.key));
            if !choices.is_empty() {
                draft.push_str(&format!("    {choices}"));
            }
        }
    }
    let label = record_type
        .map_or(plan.record_type.as_str(), |kind| kind.label.as_str());
    let message = if !plan.missing.is_empty() {
        format!(
            "a {label} note needs {} more required field{}",
            plan.missing.len(),
            if plan.missing.len() == 1 { "" } else { "s" },
        )
    } else {
        format!("the {label} note has invalid field values")
    };
    match format {
        OutputFormat::Human => {
            eprintln!("bob records new: {message}");
            eprintln!("  {draft}");
            for diagnostic in &plan.invalid {
                eprintln!("  error: {diagnostic}");
            }
        }
        OutputFormat::Json => println!(
            "{}",
            json!({
                "schema_version": SCHEMA_VERSION,
                "status": "incomplete",
                "type": plan.record_type,
                "title": plan.title,
                "path": plan.path.display().to_string(),
                "missing": plan.missing,
                "invalid": plan.invalid.iter().map(|diagnostic| json!({
                    "field": diagnostic.field,
                    "code": diagnostic.diagnostic_code(),
                    "message": diagnostic.message,
                })).collect::<Vec<_>>(),
                "draft": draft,
            })
        ),
    }
    2
}

fn add_external_records(
    vault: &Path,
    config: &RecordsConfig,
    reports: &mut [TypeReport],
) {
    let mut files = Vec::new();
    collect_markdown(vault, vault, config, &mut files);
    for report in reports.iter_mut() {
        let Some(record_type) = config.record_type(&report.id) else {
            continue;
        };
        for path in &files {
            let relative = path
                .strip_prefix(vault)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if relative.split('/').next() == Some(record_type.dir.as_str()) {
                continue;
            }
            let Ok(contents) = fs::read_to_string(path) else {
                continue;
            };
            let Some(frontmatter) =
                crate::native::projects::parse_frontmatter(&contents)
            else {
                continue;
            };
            if crate::native::projects::frontmatter_type_names(&frontmatter)
                .iter()
                .any(|name| name == &record_type.id)
            {
                report.push(
                    "error",
                    "record_outside_folder",
                    Some(&relative),
                    Some("type"),
                    format!(
                        "record type {} belongs in {}/",
                        record_type.id, record_type.dir
                    ),
                );
            }
        }
    }
}

fn collect_markdown(
    root: &Path,
    directory: &Path,
    config: &RecordsConfig,
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let Some(name) = path.file_name() else {
                continue;
            };
            if crate::native::is_always_excluded_note_directory_name(name)
                || (directory == root
                    && config.types.iter().any(|record_type| {
                        name.to_string_lossy()
                            .eq_ignore_ascii_case(&record_type.dir)
                    }))
            {
                continue;
            }
            collect_markdown(root, &path, config, files);
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
}

fn print_list(payload: &Value, config: &RecordsConfig, format: OutputFormat) {
    if format == OutputFormat::Json {
        println!("{payload}");
        return;
    }
    if config.types.is_empty() {
        println!(
            "No record types configured in {}.",
            config::config_path().display()
        );
        return;
    }
    let styler = Styler::detect();
    println!("{}", styler.bold("Record Notes"));
    for row in payload["types"].as_array().into_iter().flatten() {
        let emoji = row["emoji"].as_str().unwrap_or("");
        let label = row["label"].as_str().unwrap_or("");
        let dir = row["dir"].as_str().unwrap_or("");
        let count = row["count"].as_u64().unwrap_or(0);
        let required = row["required_fields"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        println!(
            "{} {}  {}/  {} note{}  required: {}",
            if emoji.is_empty() { "•" } else { emoji },
            styler.bold(label),
            dir,
            count,
            if count == 1 { "" } else { "s" },
            if required.is_empty() {
                "none"
            } else {
                &required
            },
        );
    }
}

fn print_show(
    payload: &Value,
    record_type: &super::schema::RecordType,
    format: OutputFormat,
) {
    if format == OutputFormat::Json {
        println!("{payload}");
        return;
    }
    let styler = Styler::detect();
    let emoji = record_type.emoji.as_deref().unwrap_or("•");
    println!(
        "{} {}  {}/ · {} note{}",
        emoji,
        styler.bold(&record_type.label),
        record_type.dir,
        payload["count"],
        if payload["count"] == 1 { "" } else { "s" }
    );
    println!("\n{}", styler.bold("Fields"));
    for (field, value) in record_type
        .fields
        .iter()
        .zip(payload["fields"].as_array().into_iter().flatten())
    {
        let kind = value["kind"].as_str().unwrap_or("text");
        let required = if field.required {
            styler.green("required")
        } else {
            styler.dim("optional")
        };
        let default = value["default"]
            .as_str()
            .map(|default| format!(" · default: {default}"))
            .unwrap_or_default();
        println!(
            "  {} {} ({kind}, {required}{}){}",
            field_label(&field.kind),
            styler.bold(&field.label),
            if field.list { ", list" } else { "" },
            default
        );
        if let Some(choices) = value["choices"]
            .as_array()
            .filter(|choices| !choices.is_empty())
        {
            let display = choices
                .iter()
                .map(|choice| {
                    format!(
                        "{} · {}",
                        choice["value"].as_str().unwrap_or(""),
                        choice["label"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("  ");
            println!("    {display}");
        }
    }
    let suggestions: Vec<_> = record_type
        .fields
        .iter()
        .zip(payload["fields"].as_array().into_iter().flatten())
        .filter(|(field, _)| field.suggest)
        .collect();
    if !suggestions.is_empty() {
        println!("\n{}", styler.bold("Common values"));
        for (field, value) in suggestions {
            let values = value["top_values"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| {
                    format!(
                        "{} ({})",
                        item["value"].as_str().unwrap_or(""),
                        item["count"]
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            if !values.is_empty() {
                println!("  {}  {values}", field.label);
            }
        }
    }
}

fn print_check(payload: &Value, config: &RecordsConfig, format: OutputFormat) {
    if format == OutputFormat::Json {
        println!("{payload}");
        return;
    }
    let reports = payload["types"].as_array().cloned().unwrap_or_default();
    let totals = &payload["summary"];
    let counts = reports
        .iter()
        .map(|report| {
            let label =
                report["label"].as_str().unwrap_or("records").to_lowercase();
            let count = report["count"].as_u64().unwrap_or(0);
            format!(
                "{count} {}{}",
                label,
                if label.ends_with('s') { "" } else { "s" }
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let summary = format!(
        "{} — {} errors, {} warnings",
        if counts.is_empty() {
            "No record types configured"
        } else {
            &counts
        },
        totals["errors"],
        totals["warnings"],
    );
    let styler = Styler::detect();
    if totals["errors"].as_u64().unwrap_or(0) == 0 {
        println!("{} {}", styler.green("✓"), summary);
    } else {
        println!("{} {}", styler.red("✗"), summary);
    }
    for report in &reports {
        let findings =
            report["findings"].as_array().cloned().unwrap_or_default();
        if findings.is_empty() {
            continue;
        }
        println!(
            "\n{}  {}",
            styler.bold(report["label"].as_str().unwrap_or("")),
            report["dir"].as_str().unwrap_or("")
        );
        let mut previous_file = String::new();
        for finding in findings {
            let file = finding["file"].as_str().unwrap_or("");
            if file != previous_file && !file.is_empty() {
                println!("  {}", styler.dim(file));
                previous_file = file.to_owned();
            }
            let severity = finding["severity"].as_str().unwrap_or("error");
            let tag = if severity == "warning" {
                styler.yellow("WARN")
            } else {
                styler.red("ERROR")
            };
            let field = finding["field"]
                .as_str()
                .map(|field| format!(" · {field}"))
                .unwrap_or_default();
            println!(
                "    {tag}{field}: {}",
                finding["message"].as_str().unwrap_or("")
            );
        }
    }
    let _ = config;
}

fn print_new_dry_run(
    plan: &super::plan::RecordCreatePlan,
    markdown: &str,
    format: OutputFormat,
) {
    if format == OutputFormat::Json {
        println!(
            "{}",
            json!({
                "schema_version": SCHEMA_VERSION,
                "status": "planned",
                "type": plan.record_type,
                "title": plan.title,
                "path": plan.path.display().to_string(),
                "markdown": markdown,
                "properties": plan.properties.iter().map(|property| json!({
                    "key": property.key,
                    "label": property.label,
                    "state": property.state.as_str(),
                    "value": property.display,
                })).collect::<Vec<_>>(),
            })
        );
    } else {
        print!("{markdown}");
    }
}

fn print_new_success(
    payload: &Value,
    plan: &super::plan::RecordCreatePlan,
    emoji: Option<&str>,
    format: OutputFormat,
) {
    if format == OutputFormat::Json {
        println!("{payload}");
        return;
    }
    println!(
        "{}  {} {}  {}",
        Styler::detect().green("✓"),
        emoji.unwrap_or(""),
        plan.path.display(),
        plan.title
    );
}

fn report_config_error(
    command: &str,
    format: OutputFormat,
    error: super::schema::RecordDiagnostic,
) -> i32 {
    report_error(command, format, &error.to_string(), 2)
}

fn report_error(
    command: &str,
    format: OutputFormat,
    message: &str,
    code: i32,
) -> i32 {
    match format {
        OutputFormat::Human => eprintln!("{command}: {message}"),
        OutputFormat::Json => println!(
            "{}",
            json!({
                "schema_version": SCHEMA_VERSION,
                "error": { "message": message },
            })
        ),
    }
    code
}

fn kind_name(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "text",
        FieldKind::Choices(_) => "choices",
        FieldKind::Date => "date",
        FieldKind::Link => "link",
        FieldKind::Url => "url",
        FieldKind::Number => "number",
    }
}

fn field_label(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "Aa",
        FieldKind::Choices(_) => "▾",
        FieldKind::Date => "◷",
        FieldKind::Link => "↗",
        FieldKind::Url => "↗",
        FieldKind::Number => "#",
    }
}

fn value_display(value: &RecordValue) -> String {
    let one = |value: &RecordScalar| match value {
        RecordScalar::Text(value)
        | RecordScalar::Choice(value)
        | RecordScalar::Link(value) => value.clone(),
        RecordScalar::Date(RecordDate::Resolved(value)) => {
            value.format("%Y-%m-%d").to_string()
        }
        RecordScalar::Date(RecordDate::Today) => "today".to_owned(),
        RecordScalar::Date(RecordDate::Yesterday) => "yesterday".to_owned(),
        RecordScalar::Url(value) => value.to_string(),
        RecordScalar::Number(value) => value.to_string(),
    };
    match value {
        RecordValue::Scalar(value) => one(value),
        RecordValue::List(values) => {
            values.iter().map(one).collect::<Vec<_>>().join(", ")
        }
    }
}

fn open_in_obsidian(vault: &Path, relative: &Path) -> Result<(), String> {
    let vault_name = vault
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!("cannot determine vault name from {}", vault.display())
        })?;
    let vault_q: String =
        url::form_urlencoded::byte_serialize(vault_name.as_bytes()).collect();
    let file_q: String = url::form_urlencoded::byte_serialize(
        relative.to_string_lossy().as_bytes(),
    )
    .collect();
    let uri = format!("obsidian://open?vault={vault_q}&file={file_q}");
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(program)
        .arg(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!("could not open the note in Obsidian: {error}")
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}
