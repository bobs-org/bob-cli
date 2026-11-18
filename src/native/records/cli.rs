//! Public `bob records` commands.

use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::{self, Read as _},
    iter,
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
    check::{collect_orphan_records, AlbumMembership, TypeReport},
    context::{
        AlbumRecordIdentity, RecordContext, RecordKindResolution,
        ResolvedRecordKind,
    },
    plan::{plan_create_with_identity, RecordCreateRequest},
    render::RecordNoteInput,
    schema::{FieldInputState, FieldKind, RecordFieldInput, RecordsConfig},
};
use crate::native::{
    albums::{
        index::IndexState,
        parse_album_declaration,
        registry::{AlbumEntry, DiscoveryHealth, RecordTypeSource},
        walk as album_walk,
        walk::WalkWarning,
        write as album_write, AlbumDeclaration, AlbumFinding,
        AlbumIssueSeverity,
    },
    capture::{write_staged_files_guarded, StagedTextFile},
    env as bob_env,
    host_catalog::ReadDependencies,
    style::Styler,
    vault_links::VaultLinkResolver,
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

pub(crate) fn run_update(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_update();
    let matches = match matches_for(&mut command, "bob records update", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    execute_update(&matches)
}

pub(crate) fn run_init(_args: Vec<OsString>) -> i32 {
    report_error(
        "bob records init",
        OutputFormat::Human,
        "record schemas now live in album notes; use `bob albums create` to create an album and its record schema",
        2,
    )
}

pub(crate) fn run_show(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_show();
    let matches = match matches_for(&mut command, "bob records show", args) {
        Ok(matches) => matches,
        Err(code) => return code,
    };
    let output_format = format_from(&matches);
    let vault = vault_from(&matches);
    let context = load_context(&vault);
    if let Some(code) =
        report_discovery_errors("bob records show", output_format, &context)
    {
        return code;
    }
    let selector = matches.get_one::<String>("type").map_or("", String::as_str);
    let kind = match resolve_usable_kind(&context, selector) {
        Ok(kind) => kind,
        Err(message) => {
            return report_error("bob records show", output_format, &message, 2)
        }
    };
    if let Some(stem) = matches.get_one::<String>("stem") {
        print_warnings(
            "bob records show",
            output_format,
            &kind_warnings(&context, selector),
        );
        return super::show::execute_show_record(
            output_format == OutputFormat::Json,
            &vault,
            &context,
            &kind,
            stem,
        );
    }
    let record_type = kind.record_type();
    let mut catalog = RecordCatalog::new(vault);
    let folder = catalog.folder(&record_type.dir);
    let fields: Vec<Value> = record_type
        .fields
        .iter()
        .map(|field| {
            let choice_values: Vec<String> = match &field.kind {
                FieldKind::Choices(choices) => {
                    choices.iter().map(|choice| choice.value.clone()).collect()
                }
                _ => Vec::new(),
            };
            let (filled, choice_counts) =
                folder.field_usage(&field.key, &choice_values);
            let top_values: Vec<Value> = if field.suggest {
                folder
                    .value_suggestions(&field.key)
                    .into_iter()
                    .take(5)
                    .map(|value| {
                        json!({
                            "value": value.insert,
                            "count": value.count,
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let line = kind.album.as_ref().and_then(|album| {
                album.schema_field_lines.get(&field.key).copied()
            });
            let mut field_json =
                field.schema_json(line, filled, &choice_counts);
            field_json["top_values"] = json!(top_values);
            field_json
        })
        .collect();
    let mut payload = json!({
        "schema_version": SCHEMA_VERSION,
        "type": record_type.id,
        "label": record_type.label,
        "dir": record_type.dir,
        "emoji": record_type.emoji,
        "dated_notes": record_type.dated_notes,
        "count": folder.notes().len(),
        "fields": fields,
    });
    merge_kind_metadata(&mut payload, &kind);
    print_warnings(
        "bob records show",
        output_format,
        &kind_warnings(&context, selector),
    );
    print_show(&payload, record_type, kind.album.as_ref(), output_format);
    0
}

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob records check")
        .about("Check Record Notes against their album schemas")
        .long_about(
            "Lint each record folder from the effective vault's album notes. Checks the type link, parent membership, required and typed fields, legacy keys, checkboxes, and records filed outside their folder. TYPE accepts a noun or album stem.",
        )
        .after_help(
            "Examples:\n  bob records check\n  bob records check restaurant\n  bob records check eat\n  bob records check -f json\n\nSee docs/records.md for the schema and lint rules.",
        )
        .disable_help_flag(true)
        .arg(record_type_arg(false))
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_list() -> ClapCommand {
    ClapCommand::new("bob records list")
        .about("List Record Note collections (default)")
        .long_about(
            "Show each album record type, its folder, note count, and required fields. Invalid albums still contribute their folder counts. This command is read-only. Bare `bob records` runs this command.",
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
        .about("Create one Record Note from its schema")
        .long_about(
            "Plan and create a single Record Note. TYPE accepts a noun or album stem. Supply fields with repeated --set KEY=VALUE arguments and notes with repeated --note arguments. Missing required fields are refused with a ready-to-copy draft. --dry-run prints the exact Markdown without writing.",
        )
        .after_help(
            "Examples:\n  bob records new restaurant 'Ramen Nagi' -s status=to_try -s category=Ramen -s location=Clark\n  bob records new eat 'Ramen Nagi' -s status=to_try -s category=Ramen -s location=Clark\n  bob records new podcast 'The Daily' -s status=to_try -s category=News -n 'Recommended by Kelly' -d\n  bob records new restaurant '1950 Originals' -S 1950s_originals -s status=to_try -s category=Pizza -s location=Westwood\n  bob records new restaurant 'Ninja Sushi' -d -f json\n\nSee docs/records.md for field kinds, defaults, and failure behavior.",
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

pub(crate) fn build_cli_update() -> ClapCommand {
    ClapCommand::new("bob records update")
        .about("Update one existing Record Note in place")
        .long_about(
            "Resolve an existing record by TYPE and STEM, then edit only the supplied YAML value spans and append notes with repeated --note arguments. TYPE accepts a noun or album stem. Field-only and note-only updates work without repeating required creation fields. With -i FILE, apply a guarded JSON envelope of field.set/field.unset ops against an expect fingerprint instead of --set and --note. --dry-run prints the exact Markdown without writing.",
        )
        .after_help(
            "Examples:\n  bob records update restaurant ninja_sushi -s status=liked -n 'Went back, still great'\n  bob records update eat ninja_sushi -s status=liked\n  bob records update restaurant ninja_sushi -s location= -d\n  bob records update podcast talk_python -n 'Listened to the async episode' -d -f json\n  bob records update eat ninja_sushi -i envelope.json -f json\n\nSee docs/records.md for update semantics, the guarded envelope, optional clearing, and failure behavior.",
        )
        .disable_help_flag(true)
        .arg(record_type_arg(true))
        .arg(
            Arg::new("stem")
                .value_name("STEM")
                .required(true)
                .value_hint(ValueHint::Other)
                .help("Existing filename stem of the record note"),
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(
            Arg::new("input")
                .short('i')
                .long("input")
                .value_name("FILE")
                .value_parser(OsStringValueParser::new())
                .conflicts_with("note")
                .conflicts_with("set")
                .help("Guarded JSON envelope file; - reads stdin"),
        )
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
                .help("Open the updated note in Obsidian"),
        )
        .arg(
            Arg::new("set")
                .short('s')
                .long("set")
                .value_name("KEY=VALUE")
                .action(ArgAction::Append)
                .help("Set a configured field; may be repeated"),
        )
}

pub(crate) fn build_cli_init() -> ClapCommand {
    ClapCommand::new("bob records init")
        .hide(true)
        .about("Retired; create an album with `bob albums create`")
}

pub(crate) fn build_cli_show() -> ClapCommand {
    ClapCommand::new("bob records show")
        .about("Show one record schema, or one record")
        .long_about(
            "Display a record type's ordered fields, kinds, required marks, defaults, choices, and most-used suggested values. With STEM, print one existing record's typed properties, identity, findings, and fingerprint. TYPE accepts a noun or album stem. This command is read-only.",
        )
        .after_help(
            "Examples:\n  bob records show restaurant\n  bob records show eat\n  bob records show zettel bead_lifecycle\n  bob records show zettel bead_lifecycle -f json\n  bob records show podcast -f json\n\nSee docs/records.md for the schema contract and reading one record.",
        )
        .disable_help_flag(true)
        .arg(record_type_arg(true))
        .arg(
            Arg::new("stem")
                .value_name("STEM")
                .required(false)
                .value_hint(ValueHint::Other)
                .help("Existing filename stem of one record note"),
        )
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
        .help("Record noun or album stem")
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

fn load_context(vault: &Path) -> RecordContext {
    RecordContext::discover(vault)
}

fn execute_list(matches: &ArgMatches) -> i32 {
    let output_format = format_from(matches);
    let vault = vault_from(matches);
    let context = load_context(&vault);
    if let Some(code) =
        report_discovery_errors("bob records list", output_format, &context)
    {
        return code;
    }
    let mut catalog = RecordCatalog::new(&vault);
    let collections = all_collections(&context);
    let rows: Vec<Value> = collections
        .iter()
        .map(|collection| {
            let folder = catalog.folder(collection.dir());
            collection.list_row(folder)
        })
        .collect();
    let payload = json!({
        "schema_version": SCHEMA_VERSION,
        "types": rows,
    });
    // Keep warning plumbing shared with route resolution.
    let mut seen_warnings = std::collections::BTreeSet::new();
    let mut list_warnings = Vec::new();
    for collection in &collections {
        let selector = match collection {
            Collection::Usable(kind) => kind.record_type().id.as_str(),
            Collection::InvalidAlbum(entry) => entry
                .declaration
                .noun_claim
                .as_deref()
                .unwrap_or(entry.declaration.stem.as_str()),
        };
        for warning in kind_warnings(&context, selector) {
            let key = format!("{}:{}", warning.code, warning.message);
            if seen_warnings.insert(key) {
                list_warnings.push(warning);
            }
        }
    }
    print_warnings("bob records list", output_format, &list_warnings);
    print_list(&payload, output_format);
    // A partial vault scan never silently reports complete: retain the
    // partial rows but fail so operators notice the missing coverage.
    if matches!(
        context.registry().health,
        crate::native::albums::registry::DiscoveryHealth::Partial(_)
    ) {
        return 1;
    }
    0
}

fn execute_check(matches: &ArgMatches) -> i32 {
    let output_format = format_from(matches);
    let vault = vault_from(matches);
    let context = load_context(&vault);
    if let Some(code) =
        report_discovery_errors("bob records check", output_format, &context)
    {
        return code;
    }
    let selected_type = matches.get_one::<String>("type").map(String::as_str);
    let collections = match selected_type {
        Some(selector) => match resolve_check_collection(&context, selector) {
            Ok(collection) => vec![collection],
            Err(message) => {
                return report_error(
                    "bob records check",
                    output_format,
                    &message,
                    2,
                )
            }
        },
        None => all_collections(&context),
    };
    let today = bob_env::current_datetime().date();
    let mut catalog = RecordCatalog::new(vault.clone());
    let resolver = VaultLinkResolver::new(&vault);
    let mut reports = Vec::new();
    for collection in &collections {
        reports.push(check_collection(
            collection,
            &vault,
            &context,
            &mut catalog,
            &resolver,
            today,
        ));
    }
    if selected_type.is_none() {
        add_orphan_records(&vault, &context, &mut reports);
    }
    let mut errors: usize = reports.iter().map(|report| report.errors).sum();
    let warnings: usize = reports.iter().map(|report| report.warnings).sum();
    // A partial vault scan never silently reports complete: retain the
    // readable partial results but fail full-inventory checks. A healthy
    // selected album is never blocked solely on unrelated scan failures.
    let partial_full_scan = selected_type.is_none()
        && matches!(
            context.registry().health,
            crate::native::albums::registry::DiscoveryHealth::Partial(_)
        );
    if partial_full_scan {
        errors += 1;
    }
    let payload = json!({
        "schema_version": SCHEMA_VERSION,
        "types": reports,
        "summary": { "errors": errors, "warnings": warnings },
    });
    print_check(&payload, output_format);
    if errors > 0 {
        1
    } else {
        0
    }
}

fn execute_new(matches: &ArgMatches) -> i32 {
    let output_format = format_from(matches);
    let vault = vault_from(matches);
    let type_id = matches.get_one::<String>("type").expect("required by clap");
    let title = matches
        .get_one::<String>("title")
        .expect("required by clap");
    let context = load_context(&vault);
    if let Some(code) =
        report_discovery_errors("bob records new", output_format, &context)
    {
        return code;
    }
    let kind = match resolve_usable_kind(&context, type_id) {
        Ok(kind) => kind,
        Err(message) => {
            return report_error("bob records new", output_format, &message, 2)
        }
    };
    print_warnings(
        "bob records new",
        output_format,
        &kind_warnings(&context, type_id),
    );
    let config = config_from_kind(&kind);
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
    let plan = match plan_create_with_identity(
        RecordCreateRequest {
            config: &config,
            record_type: &kind.record_type().id,
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
        kind.render_identity(),
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
    let mut markdown = markdown.to_owned();
    // Plan the album write set before any dry-run output, directory
    // creation, or real write, so dry-run and commit share bytes and an
    // incomplete record never stages an album edit.
    let album = match plan_album_write_for_record(
        &vault,
        &mut catalog,
        &kind,
        &[(plan.path.clone(), markdown.clone())],
        Some(&plan.path),
    ) {
        Ok(album) => album,
        Err(message) => {
            return report_error("bob records new", output_format, &message, 2);
        }
    };
    if let Some(walk) = album.as_ref()
        && let Some(edit) =
            walk.walk_edits.iter().find(|edit| edit.path == plan.path)
    {
        markdown = edit.after.clone();
    }
    let target = vault.join(&plan.path);
    if matches.get_flag("dry-run") {
        print_new_dry_run(
            &plan,
            &markdown,
            Some(&kind),
            album.as_ref(),
            output_format,
        );
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
    let mut staged = vec![StagedTextFile {
        target: target.clone(),
        target_existed: false,
        original_target: String::new(),
        updated_target: markdown.clone(),
    }];
    if let Some(album) = album.as_ref()
        && let Some(edit) = &album.edit
    {
        staged.push(StagedTextFile {
            target: vault.join(&album.relative),
            target_existed: true,
            original_target: edit.before.clone(),
            updated_target: edit.after.clone(),
        });
    }
    if let Some(album) = album.as_ref() {
        staged.extend(
            album
                .walk_edits
                .iter()
                .filter(|edit| edit.path != plan.path)
                .map(|edit| StagedTextFile {
                    target: vault.join(&edit.path),
                    target_existed: true,
                    original_target: edit.before.clone(),
                    updated_target: edit.after.clone(),
                }),
        );
    }
    let deps = album_read_dependencies(album.as_ref(), &[]);
    if let Err(error) =
        write_staged_files_guarded(&staged, || deps.validate(&vault))
    {
        return report_error(
            "bob records new",
            output_format,
            &error.message,
            2,
        );
    }

    let mut payload = json!({
        "schema_version": SCHEMA_VERSION,
        "status": "written",
        "type": plan.record_type,
        "title": plan.title,
        "path": plan.path.display().to_string(),
        "markdown": markdown,
        "walk_files": album.as_ref().map(planned_walk_files_json).unwrap_or_default(),
        "warnings": album.as_ref().map(planned_walk_warnings_json).unwrap_or_default(),
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
    merge_kind_metadata(&mut payload, &kind);
    if let Some(album) = album.as_ref() {
        merge_album_index(&mut payload, album);
    }
    print_new_success(&payload, &plan, &kind, album.as_ref(), output_format);
    if matches.get_flag("open")
        && let Err(message) = open_in_obsidian(&vault, &plan.path)
    {
        eprintln!("bob records new: warning: {message}");
    }
    0
}

fn execute_update(matches: &ArgMatches) -> i32 {
    let output_format = format_from(matches);
    let vault = vault_from(matches);
    let type_id = matches.get_one::<String>("type").expect("required by clap");
    let stem = matches.get_one::<String>("stem").expect("required by clap");
    let context = load_context(&vault);
    if let Some(code) =
        report_discovery_errors("bob records update", output_format, &context)
    {
        return code;
    }
    let kind = match resolve_usable_kind(&context, type_id) {
        Ok(kind) => kind,
        Err(message) => {
            return report_error(
                "bob records update",
                output_format,
                &message,
                2,
            )
        }
    };
    print_warnings(
        "bob records update",
        output_format,
        &kind_warnings(&context, type_id),
    );
    if matches.get_one::<OsString>("input").is_some() {
        return execute_update_envelope(
            matches,
            output_format,
            &vault,
            &context,
            &kind,
        );
    }
    let config = config_from_kind(&kind);
    let fields = match parse_field_inputs(matches) {
        Ok(fields) => fields,
        Err(message) => {
            return report_error(
                "bob records update",
                output_format,
                &message,
                2,
            );
        }
    };
    let notes: Vec<RecordNoteInput> = matches
        .get_many::<String>("note")
        .into_iter()
        .flatten()
        .map(|text| RecordNoteInput::bullet(text.clone()))
        .collect();
    let today = bob_env::current_datetime().date();
    let mut catalog = RecordCatalog::new(&vault);
    let target = match super::plan::resolve_update_target(
        &config,
        &mut catalog,
        &kind.record_type().id,
        stem,
    ) {
        Ok(target) => target,
        Err(error) => {
            return report_error(
                "bob records update",
                output_format,
                &error.to_string(),
                2,
            );
        }
    };
    let absolute = vault.join(&target.path);
    let original = match fs::read_to_string(&absolute) {
        Ok(original) => original,
        Err(error) => {
            return report_error(
                "bob records update",
                output_format,
                &format!(
                    "could not read record {}: {error}",
                    target.path.display()
                ),
                2,
            );
        }
    };
    let plan = match super::plan::plan_update(super::plan::PlanUpdateRequest {
        target: &target,
        original: &original,
        fields: &fields,
        notes: &notes,
        today,
    }) {
        Ok(plan) => plan,
        Err(error) => {
            return report_error(
                "bob records update",
                output_format,
                &error.to_string(),
                2,
            );
        }
    };
    if !plan.missing.is_empty() || !plan.invalid.is_empty() {
        return report_update_incomplete(&config, &plan, output_format);
    }
    let Some(markdown) = plan.markdown.as_deref() else {
        return report_update_incomplete(&config, &plan, output_format);
    };
    // Plan the album before the no-op early return so a no-op record
    // update still repairs a stale index. Index planning never writes.
    let album = match plan_album_write_for_record(
        &vault,
        &mut catalog,
        &kind,
        &[(plan.path.clone(), markdown.to_owned())],
        None,
    ) {
        Ok(album) => album,
        Err(message) => {
            return report_error(
                "bob records update",
                output_format,
                &message,
                1,
            );
        }
    };
    let edited_album = album.as_ref().filter(|album| album.edit.is_some());
    if matches.get_flag("dry-run") {
        print_update_dry_run(
            &config,
            &plan,
            markdown,
            Some(&kind),
            edited_album,
            output_format,
        );
        return 0;
    }
    // A current index contributes no write, even for a notes-only record
    // update; the album read guard still applies below.
    if markdown == original && edited_album.is_none() {
        print_update_success(
            &config,
            &plan,
            &notes,
            Some(&kind),
            None,
            output_format,
        );
        return 0;
    }
    let mut staged = Vec::new();
    if markdown != original {
        staged.push(StagedTextFile {
            target: absolute.clone(),
            target_existed: true,
            original_target: original.clone(),
            updated_target: markdown.to_owned(),
        });
    }
    if let Some(album) = edited_album
        && let Some(edit) = &album.edit
    {
        staged.push(StagedTextFile {
            target: vault.join(&album.relative),
            target_existed: true,
            original_target: edit.before.clone(),
            updated_target: edit.after.clone(),
        });
    }
    // Guard the album on every write, and the untouched record on an
    // album-only repair, so a concurrent edit refuses instead of
    // publishing a stale index.
    let mut extra = Vec::new();
    if markdown == original {
        extra.push((plan.path.clone(), original.clone()));
    }
    let deps = album_read_dependencies(album.as_ref(), &extra);
    if let Err(error) =
        write_staged_files_guarded(&staged, || deps.validate(&vault))
    {
        return report_error(
            "bob records update",
            output_format,
            &error.message,
            1,
        );
    }
    print_update_success(
        &config,
        &plan,
        &notes,
        Some(&kind),
        edited_album,
        output_format,
    );
    if matches.get_flag("open")
        && let Err(message) = open_in_obsidian(&vault, &plan.path)
    {
        eprintln!("bob records update: warning: {message}");
    }
    0
}

/// Typed envelope-path failure: semantic refusals exit 2, runtime and guard
/// failures exit 1.
struct EnvelopeError {
    code: String,
    message: String,
    field: Option<String>,
    hint: Option<String>,
    exit: i32,
}

impl EnvelopeError {
    fn refusal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            field: None,
            hint: None,
            exit: 2,
        }
    }

    fn runtime(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            field: None,
            hint: None,
            exit: 1,
        }
    }

    fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl From<super::protocol::ProtocolError> for EnvelopeError {
    fn from(error: super::protocol::ProtocolError) -> Self {
        let mut refusal = Self::refusal(error.code, error.to_string());
        if let Some(field) = error.field {
            refusal = refusal.with_field(field);
        }
        if let Some(hint) = error.hint {
            refusal = refusal.with_hint(hint);
        }
        refusal
    }
}

/// Guarded `bob records update -i FILE` writer: a typed JSON envelope of
/// `field.set`/`field.unset` ops applied against an `expect` fingerprint,
/// with inverse ops, group consequence, and typed refusals. Without `-i`
/// the classic `--set`/`--note` path above runs byte-identical.
fn execute_update_envelope(
    matches: &ArgMatches,
    format: OutputFormat,
    vault: &Path,
    context: &RecordContext,
    kind: &ResolvedRecordKind,
) -> i32 {
    let dry_run = matches.get_flag("dry-run");
    let type_id = matches.get_one::<String>("type").expect("required by clap");
    let stem = matches.get_one::<String>("stem").expect("required by clap");
    let config = config_from_kind(kind);
    let record_type = kind.record_type();
    let failed = |error: EnvelopeError, fingerprints: (&str, &str)| -> i32 {
        emit_envelope_refusal(
            format,
            dry_run,
            type_id,
            stem,
            "",
            fingerprints,
            error,
            Vec::new(),
            Vec::new(),
            (Value::Null, Value::Null),
            None,
        )
        .into()
    };

    let text = match read_envelope_text(matches.get_one("input")) {
        Ok(text) => text,
        Err(error) => return failed(error, ("", "")),
    };
    let envelope = match super::protocol::parse_envelope(&text) {
        Ok(envelope) => envelope,
        Err(error) => return failed(error.into(), ("", "")),
    };

    let today = bob_env::current_datetime().date();
    let mut catalog = RecordCatalog::new(vault);
    let target = match super::plan::resolve_update_target(
        &config,
        &mut catalog,
        &kind.record_type().id,
        stem,
    ) {
        Ok(target) => target,
        Err(error) => {
            return failed(
                EnvelopeError::refusal(
                    error.code.diagnostic_code(),
                    error.to_string(),
                ),
                ("", ""),
            );
        }
    };
    let failed_at_target =
        |error: EnvelopeError, fingerprints: (&str, &str)| -> EnvelopeRefusal {
            emit_envelope_refusal(
                format,
                dry_run,
                type_id,
                stem,
                &target.path.display().to_string(),
                fingerprints,
                error,
                Vec::new(),
                Vec::new(),
                (Value::Null, Value::Null),
                None,
            )
        };
    let absolute = vault.join(&target.path);
    let original = match fs::read_to_string(&absolute) {
        Ok(original) => original,
        Err(error) => {
            return failed_at_target(
                EnvelopeError::runtime(
                    "record_unreadable",
                    format!(
                        "could not read record {}: {error}",
                        target.path.display()
                    ),
                ),
                ("", ""),
            )
            .into();
        }
    };
    let (fingerprint_before, _schema_fp) =
        super::show::current_record_fingerprint(context, kind, &original);
    if envelope.expect != fingerprint_before {
        let record = match super::show::record_json_for(
            vault,
            context,
            kind,
            &target,
            &original,
            &mut catalog,
        ) {
            Ok(record) => record,
            Err(message) => {
                return failed_at_target(
                    EnvelopeError::runtime("record_unreadable", message),
                    (&fingerprint_before, &fingerprint_before),
                )
                .into();
            }
        };
        // The current record object rides along so the Record Card
        // repaints in one call.
        let group = record.get("group").cloned().unwrap_or(Value::Null);
        return emit_envelope_refusal(
            format,
            dry_run,
            type_id,
            stem,
            &target.path.display().to_string(),
            (&fingerprint_before, &fingerprint_before),
            EnvelopeError::refusal(
                "stale_record",
                format!(
                    "record {} changed on disk after the envelope was read; nothing was written",
                    target.path.display()
                ),
            )
            .with_hint(
                "re-read with `bob records show TYPE STEM -f json` and retry with the new fingerprint",
            ),
            Vec::new(),
            Vec::new(),
            (group.clone(), group),
            Some(record),
        )
        .into();
    }
    let refuse_with_plan = |error: EnvelopeError,
                            plan: &super::plan::RecordUpdatePlan|
     -> EnvelopeRefusal {
        let current =
            envelope_group_json(record_type, &plan.properties, false, today);
        let groups = (current.clone(), current);
        emit_envelope_refusal(
            format,
            dry_run,
            type_id,
            stem,
            &target.path.display().to_string(),
            (&fingerprint_before, &fingerprint_before),
            error,
            envelope_plan_properties(plan),
            envelope_plan_warnings(plan),
            groups,
            None,
        )
    };
    let inputs =
        match super::protocol::ops_to_inputs(record_type, &envelope.ops) {
            Ok(inputs) => inputs,
            Err(error) => {
                return failed_at_target(
                    error.into(),
                    (&fingerprint_before, &fingerprint_before),
                )
                .into();
            }
        };
    let plan = match super::plan::plan_update(super::plan::PlanUpdateRequest {
        target: &target,
        original: &original,
        fields: &inputs,
        notes: &[],
        today,
    }) {
        Ok(plan) => plan,
        Err(error) => {
            return failed_at_target(
                apply_envelope_error(&error),
                (&fingerprint_before, &fingerprint_before),
            )
            .into();
        }
    };
    if !plan.missing.is_empty() {
        let first = plan.missing.first().cloned().unwrap_or_default();
        let message = if plan.missing.len() == 1 {
            format!("field `{first}` is required")
        } else {
            format!(
                "{} required fields are empty: {}",
                plan.missing.len(),
                plan.missing
                    .iter()
                    .map(|key| format!("`{key}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return refuse_with_plan(
            EnvelopeError::refusal("record_required_empty", message)
                .with_field(first),
            &plan,
        )
        .into();
    }
    if !plan.invalid.is_empty() {
        let diagnostic = &plan.invalid[0];
        let hint = if diagnostic.suggestions.is_empty() {
            None
        } else {
            Some(format!(
                "did you mean {}?",
                diagnostic
                    .suggestions
                    .iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        };
        let mut error = EnvelopeError::refusal(
            diagnostic.diagnostic_code(),
            diagnostic.message.clone(),
        );
        if let Some(field) = &diagnostic.field {
            error = error.with_field(field.clone());
        }
        if let Some(hint) = hint {
            error = error.with_hint(hint);
        }
        return refuse_with_plan(error, &plan).into();
    }
    let Some(markdown) = plan.markdown.as_deref() else {
        return failed_at_target(
            EnvelopeError::runtime(
                "write_failed",
                "the planned update produced no Markdown; nothing was written",
            ),
            (&fingerprint_before, &fingerprint_before),
        )
        .into();
    };
    let (fingerprint_after, _) =
        super::show::current_record_fingerprint(context, kind, markdown);
    // Plan the album before the no-op early return so a no-op record update
    // still repairs a stale index. Index planning never writes.
    let album = match plan_album_write_for_record(
        vault,
        &mut catalog,
        kind,
        &[(plan.path.clone(), markdown.to_owned())],
        None,
    ) {
        Ok(album) => album,
        Err(message) => {
            return refuse_with_plan(
                EnvelopeError::runtime("album_stale", message),
                &plan,
            )
            .into();
        }
    };
    let edited_album = album.as_ref().filter(|album| album.edit.is_some());
    if dry_run {
        return emit_envelope_success(
            matches,
            format,
            kind,
            &plan,
            &envelope,
            &fingerprint_before,
            &fingerprint_after,
            edited_album,
            true,
        );
    }
    // A current index contributes no write; the album read guard still
    // applies below.
    if markdown == original && edited_album.is_none() {
        return emit_envelope_success(
            matches,
            format,
            kind,
            &plan,
            &envelope,
            &fingerprint_before,
            &fingerprint_after,
            None,
            false,
        );
    }
    let mut staged = Vec::new();
    if markdown != original {
        staged.push(StagedTextFile {
            target: absolute.clone(),
            target_existed: true,
            original_target: original.clone(),
            updated_target: markdown.to_owned(),
        });
    }
    if let Some(album) = edited_album
        && let Some(edit) = &album.edit
    {
        staged.push(StagedTextFile {
            target: vault.join(&album.relative),
            target_existed: true,
            original_target: edit.before.clone(),
            updated_target: edit.after.clone(),
        });
    }
    // Guard the album on every write, and the untouched record on an
    // album-only repair, so a concurrent edit refuses instead of
    // publishing a stale index.
    let mut extra = Vec::new();
    if markdown == original {
        extra.push((plan.path.clone(), original.clone()));
    }
    let deps = album_read_dependencies(album.as_ref(), &extra);
    if let Err(error) =
        write_staged_files_guarded(&staged, || deps.validate(vault))
    {
        return refuse_with_plan(
            EnvelopeError::runtime("write_failed", error.message),
            &plan,
        )
        .into();
    }
    emit_envelope_success(
        matches,
        format,
        kind,
        &plan,
        &envelope,
        &fingerprint_before,
        &fingerprint_after,
        edited_album,
        false,
    )
}

/// `properties[]` in today's shape for a refused envelope plan.
fn envelope_plan_properties(
    plan: &super::plan::RecordUpdatePlan,
) -> Vec<Value> {
    plan.properties.iter().map(update_property_json).collect()
}

/// `warnings[]` for a refused envelope plan.
fn envelope_plan_warnings(plan: &super::plan::RecordUpdatePlan) -> Vec<Value> {
    plan.warnings
        .iter()
        .map(|warning| {
            json!({
                "field": warning.field,
                "code": warning.diagnostic_code(),
                "message": warning.message,
            })
        })
        .collect()
}

/// Human output for a refused envelope plus the typed JSON document.
#[allow(clippy::too_many_arguments)]
fn emit_envelope_refusal(
    format: OutputFormat,
    dry_run: bool,
    type_id: &str,
    stem: &str,
    path: &str,
    fingerprints: (&str, &str),
    error: EnvelopeError,
    properties: Vec<Value>,
    warnings: Vec<Value>,
    groups: (Value, Value),
    record: Option<Value>,
) -> EnvelopeRefusal {
    let exit = error.exit;
    let message = error.message.clone();
    let hint = error.hint.clone();
    let mut payload = json!({
        "schema_version": SCHEMA_VERSION,
        "ok": false,
        "command": "update",
        "dry_run": dry_run,
        "type": type_id,
        "stem": stem,
        "path": path,
        "changed": Vec::<String>::new(),
        "properties": properties,
        "changes": Vec::<Value>::new(),
        "fingerprint_before": fingerprints.0,
        "fingerprint_after": fingerprints.1,
        "inverse_ops": Vec::<Value>::new(),
        "group_before": groups.0,
        "group_after": groups.1,
        "consequence": "",
        "warnings": warnings,
        "error": {
            "code": error.code,
            "message": error.message,
            "field": error.field,
            "hint": error.hint,
        },
    });
    // The stale refusal embeds the current record object so the Record
    // Card repaints in one call.
    if let Some(record) = record
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("record".to_owned(), record);
    }
    if format == OutputFormat::Json {
        println!("{payload}");
    } else {
        eprintln!("bob records update: {message}");
        if let Some(hint) = &hint {
            eprintln!("  hint: {hint}");
        }
    }
    EnvelopeRefusal { payload, exit }
}

/// Refusal outcome; the stale refusal also carries the current record
/// object inside its JSON payload.
struct EnvelopeRefusal {
    payload: Value,
    exit: i32,
}

impl From<EnvelopeRefusal> for i32 {
    fn from(refusal: EnvelopeRefusal) -> i32 {
        refusal.exit
    }
}

/// Read the JSON envelope: `-i FILE`, `-i -` for stdin.
fn read_envelope_text(
    input: Option<&OsString>,
) -> Result<String, EnvelopeError> {
    match input {
        Some(raw) if raw == "-" => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).map_err(|error| {
                EnvelopeError::refusal(
                    "envelope_unreadable",
                    format!(
                        "could not read the envelope from stdin: {error}"
                    ),
                )
                .with_hint(
                    "pass a JSON file with -i FILE or pipe the envelope on stdin",
                )
            })?;
            Ok(text)
        }
        Some(raw) => {
            let path = PathBuf::from(raw);
            fs::read_to_string(&path).map_err(|error| {
                EnvelopeError::refusal(
                    "envelope_unreadable",
                    format!(
                        "could not read input {}: {error}",
                        path.display()
                    ),
                )
                .with_hint(
                    "pass a JSON file with -i FILE or pipe the envelope on stdin",
                )
            })
        }
        None => Err(EnvelopeError::runtime(
            "envelope_unreadable",
            "the envelope branch requires -i FILE",
        )),
    }
}

/// Map a span-writer apply failure to the envelope's typed error.
fn apply_envelope_error(
    error: &super::render::RecordUpdateError,
) -> EnvelopeError {
    use super::render::RecordUpdateErrorCode as Code;
    match error.code {
        Code::RequiredEmpty => {
            let mut refusal = EnvelopeError::refusal(
                "record_required_empty",
                error.message.clone(),
            );
            if let Some(field) = &error.field {
                refusal = refusal.with_field(field.clone());
            }
            refusal
        }
        Code::InvalidValue => {
            let mut refusal = EnvelopeError::refusal(
                "record_invalid_value",
                error.message.clone(),
            );
            if let Some(field) = &error.field {
                refusal = refusal.with_field(field.clone());
            }
            refusal
        }
        Code::MissingFrontmatter
        | Code::MalformedFrontmatter
        | Code::DuplicateKey
        | Code::UnsupportedYaml => {
            let mut refusal = EnvelopeError::refusal(
                "record_uneditable",
                error.message.clone(),
            )
            .with_hint("repair the note's frontmatter, then retry");
            if let Some(field) = &error.field {
                refusal = refusal.with_field(field.clone());
            }
            refusal
        }
    }
}

/// Group identity (`{field, value, label}` or null) from the plan's current
/// values (`after == false`) or effective post-update values.
fn envelope_group_json(
    record_type: &super::schema::RecordType,
    properties: &[super::plan::PlannedUpdateProperty],
    after: bool,
    today: chrono::NaiveDate,
) -> Value {
    use crate::native::albums::grouping::group_field;
    let Some(field) = group_field(&record_type.fields) else {
        return Value::Null;
    };
    let Some(property) = properties.iter().find(|row| row.key == field.key)
    else {
        return Value::Null;
    };
    let effective = if after {
        property.value.as_ref()
    } else {
        property.current.as_ref()
    };
    let display = if after {
        property.display.as_ref()
    } else {
        property.current_display.as_ref()
    };
    let (Some(value), Some(label)) = (effective, display) else {
        return Value::Null;
    };
    // Group fields are choices-kind, so only scalar text lands here; other
    // shapes (including lists) report no group like `show` does.
    let text = match value {
        super::schema::RecordValue::Scalar(scalar) => match scalar {
            super::schema::RecordScalar::Text(text)
            | super::schema::RecordScalar::Choice(text)
            | super::schema::RecordScalar::Link(text) => text.clone(),
            super::schema::RecordScalar::Url(url) => url.to_string(),
            super::schema::RecordScalar::Number(number) => number.to_string(),
            super::schema::RecordScalar::Date(date) => date
                .resolve(today)
                .map(|resolved| resolved.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
        },
        super::schema::RecordValue::List(_) => return Value::Null,
    };
    if text.is_empty() {
        return Value::Null;
    }
    json!({
        "field": field.key,
        "value": text,
        "label": label,
    })
}

/// Changed-key rows with before/after display strings.
fn envelope_changes(plan: &super::plan::RecordUpdatePlan) -> Vec<Value> {
    plan.changed
        .iter()
        .filter_map(|key| {
            plan.properties
                .iter()
                .find(|row| &row.key == key)
                .map(|property| {
                    json!({
                        "key": property.key,
                        "label": property.label,
                        "before_display": property.current_display,
                        "after_display": property.display,
                    })
                })
        })
        .collect()
}

/// Inverse ops in reverse op order, restricted to changed fields, so Undo
/// replays with `expect` = `fingerprint_after` and `ops` = `inverse_ops`.
/// Typed priors come from the snapshot currents; absent priors unset.
fn envelope_inverse_ops(
    ops: &[super::protocol::Op],
    plan: &super::plan::RecordUpdatePlan,
) -> Vec<Value> {
    ops.iter()
        .rev()
        .filter_map(|op| {
            let field = op.field();
            if !plan.changed.iter().any(|key| key == field) {
                return None;
            }
            let current = plan
                .properties
                .iter()
                .find(|row| row.key == field)
                .and_then(|row| row.current.as_ref());
            match current {
                Some(value) => Some(json!({
                    "op": "field.set",
                    "field": field,
                    "value": crate::native::capture_record::record_value_json(value),
                })),
                None => Some(json!({
                    "op": "field.unset",
                    "field": field,
                })),
            }
        })
        .collect()
}

/// One human sentence for the status rule: what changed and where the
/// record groups now.
fn envelope_consequence(
    ops_empty: bool,
    changes: &[Value],
    group_before: &Value,
    group_after: &Value,
) -> String {
    if ops_empty {
        return "no-op: the envelope carries no operations".to_owned();
    }
    let group_suffix = if group_before != group_after {
        match group_after.get("label").and_then(Value::as_str) {
            Some(label) => format!("; moves to the {label} group"),
            None => "; leaves its group".to_owned(),
        }
    } else {
        String::new()
    };
    if changes.is_empty() {
        return "no changes: the record already matches".to_owned();
    }
    if changes.len() == 1 {
        let row = &changes[0];
        let label = row["label"].as_str().unwrap_or("field");
        let edit = match (
            row["before_display"].as_str(),
            row["after_display"].as_str(),
        ) {
            (Some(before), Some(after)) => format!("{before} → {after}"),
            (Some(before), None) => format!("cleared (was {before})"),
            (None, Some(after)) => format!("set to {after}"),
            (None, None) => "touched".to_owned(),
        };
        return format!("{label}: {edit}{group_suffix}");
    }
    let labels = changes
        .iter()
        .filter_map(|row| row["label"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} fields updated ({labels}){group_suffix}", changes.len())
}

/// Assemble and print the success/no-op/dry-run JSON document; human output
/// reuses the classic update printers.
#[allow(clippy::too_many_arguments)]
fn emit_envelope_success(
    matches: &ArgMatches,
    format: OutputFormat,
    kind: &ResolvedRecordKind,
    plan: &super::plan::RecordUpdatePlan,
    envelope: &super::protocol::Envelope,
    fingerprint_before: &str,
    fingerprint_after: &str,
    album: Option<&PlannedAlbumWrite>,
    dry_run: bool,
) -> i32 {
    let config = config_from_kind(kind);
    if format == OutputFormat::Human {
        if dry_run {
            print_update_dry_run(
                &config,
                plan,
                plan.markdown.as_deref().unwrap_or(""),
                Some(kind),
                album,
                format,
            );
            return 0;
        }
        print_update_success(&config, plan, &[], Some(kind), album, format);
        if matches.get_flag("open")
            && let Err(message) =
                open_in_obsidian(&vault_from(matches), &plan.path)
        {
            eprintln!("bob records update: warning: {message}");
        }
        return 0;
    }
    let changes = envelope_changes(plan);
    let record_type = kind.record_type();
    let today = bob_env::current_datetime().date();
    let group_before =
        envelope_group_json(record_type, &plan.properties, false, today);
    let group_after =
        envelope_group_json(record_type, &plan.properties, true, today);
    let mut consequence = envelope_consequence(
        envelope.ops.is_empty(),
        &changes,
        &group_before,
        &group_after,
    );
    if dry_run {
        consequence = format!("would apply: {consequence}");
    }
    let mut payload = json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "command": "update",
        "dry_run": dry_run,
        "type": plan.record_type,
        "stem": plan.stem,
        "path": plan.path.display().to_string(),
        "changed": plan.changed,
        "properties": plan.properties.iter().map(update_property_json).collect::<Vec<_>>(),
        "changes": changes,
        "fingerprint_before": fingerprint_before,
        "fingerprint_after": fingerprint_after,
        "inverse_ops": envelope_inverse_ops(&envelope.ops, plan),
        "group_before": group_before,
        "group_after": group_after,
        "consequence": consequence,
        "warnings": plan.warnings.iter().map(|warning| json!({
            "field": warning.field,
            "code": warning.diagnostic_code(),
            "message": warning.message,
        })).collect::<Vec<_>>(),
        "error": Value::Null,
    });
    if let Some(album) = album {
        merge_album_index(&mut payload, album);
    }
    merge_kind_metadata(&mut payload, kind);
    println!("{payload}");
    if matches.get_flag("open")
        && let Err(message) = open_in_obsidian(&vault_from(matches), &plan.path)
    {
        eprintln!("bob records update: warning: {message}");
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

fn report_update_incomplete(
    config: &RecordsConfig,
    plan: &super::plan::RecordUpdatePlan,
    format: OutputFormat,
) -> i32 {
    let record_type = config.record_type(&plan.record_type);
    let mut draft = format!(
        "@{}/{}",
        record_type.map_or("", |kind| kind.dir.as_str()),
        plan.stem
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
            eprintln!("bob records update: {message}");
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
                "stem": plan.stem,
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

fn update_property_json(
    property: &super::plan::PlannedUpdateProperty,
) -> Value {
    let value = property
        .value
        .as_ref()
        .map(crate::native::capture_record::record_value_json);
    let before = property
        .current
        .as_ref()
        .map(crate::native::capture_record::record_value_json);
    let source = property.source.map(|source| match source {
        super::plan::RecordPropertySource::Typed => "typed",
        super::plan::RecordPropertySource::Default => "default",
    });
    json!({
        "key": property.key,
        "label": property.label,
        "state": property.state.as_str(),
        "value": value,
        "display": property.display,
        "before": before,
        "before_display": property.current_display,
        "source": source,
    })
}

fn update_warning_text(warning: &super::schema::RecordDiagnostic) -> String {
    let field = warning
        .field
        .as_deref()
        .map(|field| format!(" `{field}`"))
        .unwrap_or_default();
    format!(
        "existing record{field}: {}; review it with `bob records check`",
        warning.message
    )
}

fn print_update_dry_run(
    _config: &RecordsConfig,
    plan: &super::plan::RecordUpdatePlan,
    markdown: &str,
    kind: Option<&ResolvedRecordKind>,
    album: Option<&PlannedAlbumWrite>,
    format: OutputFormat,
) {
    if format == OutputFormat::Json {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "status": "planned",
            "type": plan.record_type,
            "title": plan.title,
            "stem": plan.stem,
            "path": plan.path.display().to_string(),
            "markdown": markdown,
            "body": plan.body,
            "changed": plan.changed,
            "properties": plan.properties.iter().map(update_property_json).collect::<Vec<_>>(),
            "warnings": plan.warnings.iter().map(|warning| json!({
                "field": warning.field,
                "code": warning.diagnostic_code(),
                "message": warning.message,
            })).collect::<Vec<_>>(),
        });
        if let Some(kind) = kind {
            merge_kind_metadata(&mut payload, kind);
        }
        if let Some(album) = album {
            merge_album_index(&mut payload, album);
        }
        println!("{payload}");
        return;
    }
    print!("{markdown}");
    if let Some(album) = album
        && let Some(edit) = &album.edit
    {
        print_album_dry_run(album, &edit.after);
    }
}

fn print_update_success(
    config: &RecordsConfig,
    plan: &super::plan::RecordUpdatePlan,
    notes: &[RecordNoteInput],
    kind: Option<&ResolvedRecordKind>,
    album: Option<&PlannedAlbumWrite>,
    format: OutputFormat,
) {
    let record_type = config.record_type(&plan.record_type);
    let emoji = record_type.and_then(|kind| kind.emoji.as_deref());
    let label = record_type
        .map_or(plan.record_type.as_str(), |kind| kind.label.as_str());
    if format == OutputFormat::Json {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "status": "written",
            "type": plan.record_type,
            "title": plan.title,
            "stem": plan.stem,
            "path": plan.path.display().to_string(),
            "changed": plan.changed,
            "notes": notes.len(),
            "properties": plan.properties.iter().map(update_property_json).collect::<Vec<_>>(),
            "warnings": plan.warnings.iter().map(|warning| json!({
                "field": warning.field,
                "code": warning.diagnostic_code(),
                "message": warning.message,
            })).collect::<Vec<_>>(),
        });
        if let Some(kind) = kind {
            merge_kind_metadata(&mut payload, kind);
        }
        if let Some(album) = album {
            merge_album_index(&mut payload, album);
        }
        println!("{payload}");
    } else {
        let prefix = Styler::detect().green("✓");
        let emoji_text =
            emoji.map(|emoji| format!("{emoji} ")).unwrap_or_default();
        println!(
            "{}  {}updated {}  {}",
            prefix,
            emoji_text,
            label.to_lowercase(),
            plan.path.display(),
        );
        for property in &plan.properties {
            let Some(display) = property.display.as_deref() else {
                continue;
            };
            if property.current_display.as_deref() == Some(display)
                && property.source.is_none()
            {
                continue;
            }
            if let Some(before) = property.current_display.as_deref()
                && before != display
            {
                println!(
                    "  {}  {before} → {display}",
                    Styler::detect().dim(&property.label)
                );
                continue;
            }
            println!(
                "  {}  {}",
                Styler::detect().dim(&property.label),
                display
            );
        }
        if !notes.is_empty() {
            let word = if notes.len() == 1 { "note" } else { "notes" };
            println!(
                "  {}",
                Styler::detect().dim(&format!("{} {word}", notes.len()))
            );
        }
        if let Some(album) = album
            && let Some(_) = &album.edit
        {
            println!(
                "  {}  {}",
                Styler::detect().dim("index"),
                album.relative.display(),
            );
        }
    }
    for warning in &plan.warnings {
        let styler = Styler::detect();
        eprintln!(
            "bob records update: {}: {}",
            styler.warning_prefix(),
            update_warning_text(warning)
        );
    }
    let _ = config;
}

fn print_list(payload: &Value, format: OutputFormat) {
    if format == OutputFormat::Json {
        println!("{payload}");
        return;
    }
    let rows = payload["types"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("No record types in the effective vault.");
        return;
    }
    let styler = Styler::detect();
    println!("{}", styler.bold("Record Notes"));
    for row in &rows {
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
        let album = album_owner_suffix(row);
        println!(
            "{} {}  {}/  {} note{}  required: {}{}",
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
            album,
        );
    }
}

fn print_show(
    payload: &Value,
    record_type: &super::schema::RecordType,
    album: Option<&AlbumRecordIdentity>,
    format: OutputFormat,
) {
    if format == OutputFormat::Json {
        println!("{payload}");
        return;
    }
    let styler = Styler::detect();
    let emoji = record_type.emoji.as_deref().unwrap_or("•");
    let album_text = album
        .map(|album| match album.owner.as_deref() {
            Some(owner) => format!("  {} · {owner}", album.title),
            None => format!("  {}", album.title),
        })
        .unwrap_or_default();
    println!(
        "{} {}  {}/ · {} note{}{album_text}",
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

fn print_check(payload: &Value, format: OutputFormat) {
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
            "No record types"
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
}

fn print_new_dry_run(
    plan: &super::plan::RecordCreatePlan,
    markdown: &str,
    kind: Option<&ResolvedRecordKind>,
    album: Option<&PlannedAlbumWrite>,
    format: OutputFormat,
) {
    if format == OutputFormat::Json {
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "status": "planned",
            "type": plan.record_type,
            "title": plan.title,
            "path": plan.path.display().to_string(),
            "markdown": markdown,
            "walk_files": album.map(planned_walk_files_json).unwrap_or_default(),
            "warnings": album.map(planned_walk_warnings_json).unwrap_or_default(),
            "properties": plan.properties.iter().map(|property| json!({
                "key": property.key,
                "label": property.label,
                "state": property.state.as_str(),
                "value": property.display,
            })).collect::<Vec<_>>(),
        });
        if let Some(kind) = kind {
            merge_kind_metadata(&mut payload, kind);
        }
        if let Some(album) = album {
            merge_album_index(&mut payload, album);
        }
        println!("{payload}");
    } else {
        print!("{markdown}");
        if let Some(album) = album
            && let Some(edit) = &album.edit
        {
            print_album_dry_run(album, &edit.after);
        }
        if let Some(album) = album {
            print_walk_files_human(album);
            print_walk_warnings_human(album);
        }
    }
}

fn planned_walk_files_json(album: &PlannedAlbumWrite) -> Vec<Value> {
    album
        .walk_files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "before": file.before,
                "after": file.after,
            })
        })
        .collect()
}

fn planned_walk_warnings_json(album: &PlannedAlbumWrite) -> Vec<Value> {
    album
        .walk_warnings
        .iter()
        .map(|warning| {
            json!({
                "code": "record_walk_damaged",
                "path": warning.path,
                "reason": warning.reason.as_str(),
                "message": format!(
                    "{}: left damaged record walk line untouched ({})",
                    warning.path,
                    warning.reason.as_str(),
                ),
            })
        })
        .collect()
}

fn print_walk_files_human(album: &PlannedAlbumWrite) {
    let styler = Styler::detect();
    let album_stem = album
        .relative
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    for file in &album.walk_files {
        println!("  {}  {}", styler.dim("walk"), file.path);
        if let Some(line) = walk_line_for_record(&file.after, album_stem) {
            println!("    {line}");
        }
    }
}

fn print_walk_warnings_human(album: &PlannedAlbumWrite) {
    for warning in &album.walk_warnings {
        eprintln!(
            "bob records new: warning: {}: left damaged record walk line untouched ({})",
            warning.path,
            warning.reason.as_str(),
        );
    }
}

fn walk_line_for_record<'a>(
    markdown: &'a str,
    album_stem: &str,
) -> Option<&'a str> {
    let hub = format!("[[{album_stem}|↑ ");
    markdown.lines().find(|line| line.contains(&hub))
}

/// Show a planned album index change alongside record dry-run output.
/// Dry-run writes nothing; this is the unified original-to-final diff a
/// real run stages.
fn print_album_dry_run(album: &PlannedAlbumWrite, after: &str) {
    let styler = Styler::detect();
    let path = album.relative.display().to_string().replace('\\', "/");
    println!(
        "\n{}  {} ({})",
        styler.dim("index"),
        album.relative.display(),
        album.state.as_str(),
    );
    let before = album
        .edit
        .as_ref()
        .map(|edit| edit.before.as_str())
        .unwrap_or(&album.before);
    print!(
        "{}",
        similar::TextDiff::from_lines(before, after)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string()
    );
}

fn print_new_success(
    payload: &Value,
    plan: &super::plan::RecordCreatePlan,
    kind: &ResolvedRecordKind,
    album: Option<&PlannedAlbumWrite>,
    format: OutputFormat,
) {
    if format == OutputFormat::Json {
        println!("{payload}");
        return;
    }
    let emoji = kind.record_type().emoji.as_deref().unwrap_or("");
    let prefix = Styler::detect().green("✓");
    if let Some(identity) = &kind.album {
        let owner = identity
            .owner
            .as_deref()
            .map(|owner| format!(" · {owner}"))
            .unwrap_or_default();
        println!(
            "{}  {emoji} New {} → {}{owner}",
            prefix,
            kind.record_type().id,
            identity.title
        );
        println!("  {}  {}", plan.path.display(), plan.title);
    } else {
        println!(
            "{}  {emoji} {}  {}",
            prefix,
            plan.path.display(),
            plan.title
        );
    }
    if let Some(album) = album
        && let Some(_) = &album.edit
    {
        println!(
            "  {}  {}",
            Styler::detect().dim("index"),
            album.relative.display(),
        );
    }
    if let Some(album) = album {
        print_walk_files_human(album);
        print_walk_warnings_human(album);
    }
}

pub(crate) fn config_from_kind(kind: &ResolvedRecordKind) -> RecordsConfig {
    RecordsConfig {
        types: vec![kind.record_type().clone()],
    }
}

/// One album's planned records-index write for a record command. `edit`
/// is `Some` only when the index must change; the `before` preimage
/// always guards the write, even for a current index.
struct PlannedAlbumWrite {
    relative: PathBuf,
    before: String,
    edit: Option<PlannedAlbumEdit>,
    state: IndexState,
    walk_edits: Vec<crate::native::host_catalog::FileEdit>,
    walk_files: Vec<PlannedWalkFile>,
    walk_warnings: Vec<WalkWarning>,
}

struct PlannedWalkFile {
    path: String,
    before: String,
    after: String,
}

struct PlannedAlbumEdit {
    before: String,
    after: String,
}

/// Plan the album's records index from the same preimage the record
/// schema was bound to. Usable record kinds all come from albums.
/// Returns a stale-note message when the album changed after kind
/// resolution; callers write nothing in that case.
fn plan_album_write_for_record(
    vault: &Path,
    catalog: &mut RecordCatalog,
    kind: &ResolvedRecordKind,
    overlays: &[(PathBuf, String)],
    created_path: Option<&Path>,
) -> Result<Option<PlannedAlbumWrite>, String> {
    let RecordTypeSource::Album { path } = &kind.sourced.source;
    let record_type = kind.record_type();
    let album_relative = PathBuf::from(path);
    let before = match fs::read_to_string(vault.join(&album_relative)) {
        Ok(before) => before,
        Err(error) => {
            return Err(format!(
                "refusing to update records index in {path}: album could not be read: {error}"
            ));
        }
    };
    let declaration = match parse_album_declaration(&album_relative, &before) {
        Some(declaration) => declaration,
        None => {
            return Err(format!(
                "refusing to update records index in {path}: album is no longer a readable album declaration"
            ));
        }
    };
    // Bind the schema to this same preimage: a declaration change after
    // kind resolution refuses instead of rendering with a stale schema.
    if declaration.record_type.as_ref() != Some(record_type) {
        return Err(format!(
            "refusing to update records index in {path}: album changed on disk after planning"
        ));
    }
    let folder = catalog.folder(&record_type.dir);
    let planned =
        album_write::plan_album_write(&declaration, &before, folder, overlays);
    if planned.state == IndexState::Damaged {
        let reason = planned
            .findings
            .first()
            .map(|finding| finding.message.clone())
            .unwrap_or_else(|| {
                format!("{path}: records index is damaged; refusing to write")
            });
        return Err(reason);
    }
    let created: HashSet<String> = created_path
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .into_iter()
        .collect();
    let walk = created_path.map(|_| {
        album_write::plan_album_walk_radius(
            &declaration,
            folder,
            overlays,
            created.clone(),
            |relative| fs::read_to_string(vault.join(relative)).ok(),
        )
    });
    let (walk_edits, walk_files, walk_warnings) = match walk {
        Some(walk) => {
            let mut files: Vec<_> = walk
                .edits
                .iter()
                .filter(|edit| {
                    !created.contains(
                        &edit.path.to_string_lossy().replace('\\', "/"),
                    )
                })
                .map(|edit| PlannedWalkFile {
                    path: edit.path.to_string_lossy().replace('\\', "/"),
                    before: edit.before.clone(),
                    after: edit.after.clone(),
                })
                .collect();
            files.sort_by(|left, right| left.path.cmp(&right.path));
            (walk.edits, files, walk.warnings)
        }
        None => (Vec::new(), Vec::new(), Vec::new()),
    };
    Ok(Some(PlannedAlbumWrite {
        relative: album_relative,
        before,
        edit: planned.edit.map(|edit| PlannedAlbumEdit {
            before: edit.before,
            after: edit.after,
        }),
        state: planned.state,
        walk_edits,
        walk_files,
        walk_warnings,
    }))
}

/// Guard every read input the album plan consumed. The guarded writer
/// validates these at both commit points, so an album edit that landed
/// after planning refuses instead of publishing a stale index.
fn album_read_dependencies(
    album: Option<&PlannedAlbumWrite>,
    extra: &[(PathBuf, String)],
) -> ReadDependencies {
    let mut deps = ReadDependencies::default();
    if let Some(album) = album {
        deps.insert(album.relative.clone(), Some(album.before.clone()));
    }
    for (relative, expected) in extra {
        deps.insert(relative.clone(), Some(expected.clone()));
    }
    deps
}

fn album_index_json(album: &PlannedAlbumWrite) -> Value {
    let edit = album.edit.as_ref().expect("album_index is planned only");
    json!({
        "path": album.relative.display().to_string(),
        "state": album.state.as_str(),
        "before": edit.before,
        "after": edit.after,
    })
}

pub(crate) fn merge_kind_metadata(
    payload: &mut Value,
    kind: &ResolvedRecordKind,
) {
    if let Some(object) = payload.as_object_mut() {
        object.insert("source".to_owned(), json!(kind.source_name()));
        if let Some(album) = &kind.album {
            object.insert(
                "album".to_owned(),
                serde_json::to_value(album).expect("album identity"),
            );
        }
    }
}

/// Add the planned album index change to a record payload. Present only
/// when the index actually changes, with the path and exact before/after
/// bytes; the record's own keys are untouched.
fn merge_album_index(payload: &mut Value, album: &PlannedAlbumWrite) {
    if album.edit.is_some()
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("album_index".to_owned(), album_index_json(album));
    }
}

fn album_identity(entry: &AlbumEntry) -> AlbumRecordIdentity {
    AlbumRecordIdentity {
        stem: entry.declaration.stem.clone(),
        title: entry.declaration.title.clone(),
        owner: entry.owner.clone(),
        breadcrumb: entry.breadcrumb.clone(),
        schema_field_lines: entry.declaration.schema_field_lines.clone(),
    }
}

fn album_owner_suffix(row: &Value) -> String {
    let Some(album) = row.get("album") else {
        return String::new();
    };
    let title = album["title"].as_str().unwrap_or("");
    if title.is_empty() {
        return String::new();
    }
    match album["owner"].as_str() {
        Some(owner) => format!("  {title} · {owner}"),
        None => format!("  {title}"),
    }
}

enum Collection {
    Usable(ResolvedRecordKind),
    InvalidAlbum(AlbumEntry),
}

impl Collection {
    fn dir(&self) -> &str {
        match self {
            Self::Usable(kind) => kind.record_type().dir.as_str(),
            Self::InvalidAlbum(entry) => entry.declaration.stem.as_str(),
        }
    }

    fn list_row(&self, folder: &super::catalog::FolderIndex) -> Value {
        match self {
            Self::Usable(kind) => {
                let record_type = kind.record_type();
                let required: Vec<&str> = record_type
                    .fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.key.as_str())
                    .collect();
                let mut row = json!({
                    "id": record_type.id,
                    "label": record_type.label,
                    "dir": record_type.dir,
                    "emoji": record_type.emoji,
                    "count": folder.notes().len(),
                    "required_fields": required,
                });
                merge_kind_metadata(&mut row, kind);
                row
            }
            Self::InvalidAlbum(entry) => {
                let id = entry
                    .declaration
                    .noun_claim
                    .clone()
                    .unwrap_or_else(|| entry.declaration.stem.clone());
                let label = entry
                    .declaration
                    .label
                    .clone()
                    .unwrap_or_else(|| entry.declaration.title.clone());
                json!({
                    "id": id,
                    "label": label,
                    "dir": entry.declaration.stem,
                    "emoji": entry.declaration.emoji,
                    "count": folder.notes().len(),
                    "required_fields": Vec::<String>::new(),
                    "source": "album",
                    "album": album_identity(entry),
                })
            }
        }
    }
}

fn all_collections(context: &RecordContext) -> Vec<Collection> {
    let usable = context.usable_kinds();
    let mut collections = Vec::new();
    for entry in context.album_claims() {
        if let Some(kind) =
            usable.iter().find(|kind| match &kind.sourced.source {
                RecordTypeSource::Album { path } => {
                    path == &entry.declaration.path
                }
            })
        {
            collections.push(Collection::Usable(kind.clone()));
        } else {
            collections.push(Collection::InvalidAlbum(entry.clone()));
        }
    }
    collections
}

fn resolve_usable_kind(
    context: &RecordContext,
    selector: &str,
) -> Result<ResolvedRecordKind, String> {
    match context.resolve(selector) {
        RecordKindResolution::Valid { kind, .. } => Ok(kind),
        RecordKindResolution::Invalid {
            paths, findings, ..
        } => Err(invalid_claim_message(selector, &paths, &findings)),
        RecordKindResolution::Unclaimed { .. } => {
            Err(unclaimed_message(selector))
        }
        RecordKindResolution::DiscoveryIncomplete { errors } => {
            Err(errors.join("\n"))
        }
    }
}

fn resolve_check_collection(
    context: &RecordContext,
    selector: &str,
) -> Result<Collection, String> {
    match context.resolve(selector) {
        RecordKindResolution::Valid { kind, .. } => {
            Ok(Collection::Usable(kind))
        }
        RecordKindResolution::Invalid {
            paths, findings, ..
        } => {
            if paths.len() != 1 {
                return Err(invalid_claim_message(selector, &paths, &findings));
            }
            context
                .album_claims()
                .iter()
                .find(|entry| entry.declaration.path == paths[0])
                .cloned()
                .map(Collection::InvalidAlbum)
                .ok_or_else(|| {
                    invalid_claim_message(selector, &paths, &findings)
                })
        }
        RecordKindResolution::Unclaimed { .. } => {
            Err(unclaimed_message(selector))
        }
        RecordKindResolution::DiscoveryIncomplete { errors } => {
            Err(errors.join("\n"))
        }
    }
}

fn invalid_claim_message(
    selector: &str,
    paths: &[String],
    findings: &[AlbumFinding],
) -> String {
    let details = if findings.is_empty() {
        format!("claimed by {}", paths.join(", "))
    } else {
        findings
            .iter()
            .map(format_album_finding)
            .collect::<Vec<_>>()
            .join("; ")
    };
    format!("record type `{selector}` is not usable: {details}")
}

fn unclaimed_message(selector: &str) -> String {
    format!("unknown record type `{selector}`")
}

fn format_album_finding(finding: &AlbumFinding) -> String {
    let mut location = finding.path.clone();
    if let Some(line) = finding.line_number {
        location.push(':');
        location.push_str(&line.to_string());
    }
    if let Some(key) = &finding.key_path {
        location.push_str(": ");
        location.push_str(key);
    }
    format!("{location}: {}", finding.message)
}

fn kind_warnings(context: &RecordContext, selector: &str) -> Vec<AlbumFinding> {
    match context.resolve(selector) {
        RecordKindResolution::Valid { warnings, .. } => warnings,
        _ => Vec::new(),
    }
}

fn print_warnings(
    command: &str,
    _format: OutputFormat,
    warnings: &[AlbumFinding],
) {
    // Emit to stderr for both human and JSON so machine payloads keep their
    // meaning while operators still see the deprecation. Exit codes unchanged.
    let styler = Styler::detect();
    for warning in warnings {
        eprintln!(
            "{command}: {}: {}",
            styler.warning_prefix(),
            format_album_finding(warning)
        );
    }
}

fn report_discovery_errors(
    command: &str,
    _format: OutputFormat,
    context: &RecordContext,
) -> Option<i32> {
    // Never block a healthy selected album on an unrelated partial scan:
    // emit to stderr for both formats and let callers decide failure status
    // from their own scope. Inventory callers surface the failure below.
    if let DiscoveryHealth::Partial(errors) = &context.registry().health {
        for error in errors {
            eprintln!("{command}: {error}");
        }
    }
    None
}

fn check_collection(
    collection: &Collection,
    vault: &Path,
    context: &super::context::RecordContext,
    catalog: &mut RecordCatalog,
    resolver: &VaultLinkResolver,
    today: chrono::NaiveDate,
) -> TypeReport {
    match collection {
        Collection::Usable(kind) => {
            let record_type = kind.record_type();
            let folder = catalog.folder(&record_type.dir);
            let mut report = TypeReport::new(record_type, folder)
                .with_source(kind.source_name())
                .with_album(kind.album.clone());
            let RecordTypeSource::Album { path } = &kind.sourced.source;
            let membership = Some(AlbumMembership {
                album_path: Path::new(path),
                stem: record_type.dir.as_str(),
                noun: record_type.id.as_str(),
                resolver,
            });
            let notes = folder.notes().to_vec();
            for note in &notes {
                let contents =
                    fs::read_to_string(vault.join(&note.relative_path)).ok();
                report.check_note(
                    record_type,
                    note,
                    today,
                    membership,
                    contents.as_deref(),
                );
            }
            report.check_suggested_values(record_type, folder);
            if !vault.join(path).is_file() {
                report.push(
                    "error",
                    "record_missing_collection_note",
                    Some(path),
                    None,
                    format!("required album note is missing: {path}"),
                );
            }
            let declaration = context
                .album_claims()
                .iter()
                .find(|entry| entry.declaration.path == *path)
                .map(|entry| entry.declaration.clone());
            if let Some(declaration) = declaration {
                push_walk_findings(vault, &declaration, folder, &mut report);
            }
            report
        }
        Collection::InvalidAlbum(entry) => {
            let dir = entry.declaration.stem.as_str();
            let folder = catalog.folder(dir);
            let id = entry
                .declaration
                .noun_claim
                .clone()
                .unwrap_or_else(|| entry.declaration.stem.clone());
            let label = entry
                .declaration
                .label
                .clone()
                .unwrap_or_else(|| entry.declaration.title.clone());
            let mut report = TypeReport::for_folder(id, label, dir, folder)
                .with_source("album")
                .with_album(Some(album_identity(entry)));
            let noun = entry.declaration.noun_claim.as_deref().unwrap_or(dir);
            let album_path = Path::new(&entry.declaration.path);
            let membership = AlbumMembership {
                album_path,
                stem: dir,
                noun,
                resolver,
            };
            let notes = folder.notes().to_vec();
            for note in &notes {
                let contents =
                    fs::read_to_string(vault.join(&note.relative_path)).ok();
                report.check_album_membership(
                    note,
                    membership,
                    contents.as_deref(),
                );
            }
            for finding in &entry.findings {
                if finding.severity != AlbumIssueSeverity::Error {
                    continue;
                }
                report.push(
                    "error",
                    finding.code,
                    Some(&finding.path),
                    finding.key_path.as_deref(),
                    finding.message.clone(),
                );
            }
            push_walk_findings(vault, &entry.declaration, folder, &mut report);
            report
        }
    }
}

/// Read-only walk health for `records check`: one `record_walk_damaged`
/// warning per damaged record and one `record_walk_stale` warning per album
/// with missing or stale lines. Both are warnings, so the exit code is
/// unchanged; `bob albums sync` is the repair owner.
fn push_walk_findings(
    vault: &Path,
    declaration: &AlbumDeclaration,
    folder: &super::catalog::FolderIndex,
    report: &mut super::check::TypeReport,
) {
    if !album_write::folder_complete(folder) {
        return;
    }
    let members = album_write::overlay_folder_members(folder, &[]);
    if members.is_empty() {
        return;
    }
    let mut texts: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for member in &members {
        let slash = member.relative_path.to_string_lossy().replace('\\', "/");
        let Ok(text) = fs::read_to_string(vault.join(&member.relative_path))
        else {
            return;
        };
        texts.insert(slash, text);
    }
    let plan = album_walk::plan_album_walk(
        declaration,
        folder,
        &[],
        |path| texts.get(path).cloned(),
        album_walk::WalkScope::All,
    );
    if matches!(
        plan.state,
        album_walk::AlbumWalkState::Held
            | album_walk::AlbumWalkState::Empty
            | album_walk::AlbumWalkState::Current
    ) {
        return;
    }
    let mut stale = 0usize;
    for record in &plan.records {
        match record.state {
            album_walk::WalkLineState::Damaged => {
                let reason = record
                    .reason
                    .map(|reason| reason.as_str())
                    .unwrap_or("damaged");
                report.push(
                    "warning",
                    "record_walk_damaged",
                    Some(&record.slot.member.path),
                    None,
                    format!(
                        "walk line {reason} in {}; delete the line and run `bob albums sync`",
                        record.slot.member.path,
                    ),
                );
            }
            album_walk::WalkLineState::Missing
            | album_walk::WalkLineState::Stale => {
                stale += 1;
            }
            album_walk::WalkLineState::Current => {}
        }
    }
    if stale > 0 {
        report.push(
            "warning",
            "record_walk_stale",
            None,
            None,
            format!(
                "{stale} record(s) need walk lines in {}; run `bob albums sync`",
                declaration.stem,
            ),
        );
    }
}

fn add_orphan_records(
    vault: &Path,
    context: &RecordContext,
    reports: &mut Vec<TypeReport>,
) {
    let album_dirs = context
        .album_claims()
        .iter()
        .map(|entry| entry.declaration.stem.clone())
        .collect();
    let orphans = collect_orphan_records(vault, &album_dirs);
    if orphans.is_empty() {
        return;
    }
    let mut report = TypeReport::orphan_bucket();
    report.count = orphans.len();
    for path in orphans {
        let relative = path
            .strip_prefix(vault)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        report.push(
            "error",
            "record_outside_album",
            Some(&relative),
            Some("type"),
            "type [[record]] belongs in an album folder".to_owned(),
        );
    }
    reports.push(report);
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

pub(crate) fn open_in_obsidian(
    vault: &Path,
    relative: &Path,
) -> Result<(), String> {
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
