use std::{
    env,
    ffi::{OsStr, OsString},
    iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::env as bob_env;

const COMMAND_NAME: &str = "bob highlights-ref";
const DEFAULT_LIB_DIR: &str = "lib";
const DEFAULT_REF_DIR: &str = "ref";
const DEFAULT_PARENT: &str = "[[obsidian]]";

const ENV_LIB_DIR: &str = "BOB_HIGHLIGHTS_LIB_DIR";
const ENV_REF_DIR: &str = "BOB_HIGHLIGHTS_REF_DIR";
const ENV_DEFAULT_PARENT: &str = "BOB_HIGHLIGHTS_DEFAULT_PARENT";

const MARKER_REQUIRED_KEY: &str = "status";
const MANAGED_BODY_BEGIN: &str = "<!-- highlights:begin -->";
const MANAGED_BODY_END: &str = "<!-- highlights:end -->";

const PIPELINE_FIELDS: &[&str] = &[
    "source_pdf",
    "source_pdf_sha256",
    "highlights_sidecar",
    "highlights_count",
    "highlights_synced_at",
    "highlights_marker_hash",
    "highlights_marker_fields",
    "pipeline_version",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    bob_dir: PathBuf,
    lib_dir: PathBuf,
    ref_dir: PathBuf,
    default_parent: String,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let matches = match build_cli().try_get_matches_from(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let exit_code = error.exit_code();
            if let Err(print_error) = error.print() {
                eprintln!(
                    "{COMMAND_NAME}: failed to print command-line error: {print_error}"
                );
            }
            return exit_code;
        }
    };

    match matches.subcommand() {
        Some(("scan", sub_matches)) => run_scan(sub_matches),
        Some(("sync", sub_matches)) => run_sync(sub_matches),
        Some(("doctor", sub_matches)) => run_doctor(sub_matches),
        Some(("marker", sub_matches)) => run_marker(sub_matches),
        _ => 2,
    }
}

fn run_scan(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    print_config_report("scan", &config);
    println!("dry_run: {}", matches.get_flag("dry-run"));
    print_phase_one_no_write_notice();
    0
}

fn run_sync(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let pdf = required_path(matches, "pdf");
    print_config_report("sync", &config);
    println!("pdf: {}", pdf.display());
    println!("dry_run: {}", matches.get_flag("dry-run"));
    println!("write_pdf: {}", matches.get_flag("write-pdf"));
    if let Some(prefer) = matches.get_one::<String>("prefer") {
        println!("prefer: {prefer}");
    }
    print_phase_one_no_write_notice();
    0
}

fn run_doctor(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    print_config_report("doctor", &config);
    println!("checks: pending in a later phase");
    print_phase_one_no_write_notice();
    0
}

fn run_marker(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let pdf = required_path(matches, "pdf");
    print_config_report("marker", &config);
    println!("pdf: {}", pdf.display());
    println!("marker_required_key: {MARKER_REQUIRED_KEY}");
    println!("marker_items: '- key: value' or '* key: value'");
    print_phase_one_no_write_notice();
    0
}

fn print_config_report(operation: &str, config: &Config) {
    println!("Highlights reference sync");
    println!("operation: {operation}");
    println!("bob_dir: {}", config.bob_dir.display());
    println!("lib_dir: {}", config.lib_dir.display());
    println!("ref_dir: {}", config.ref_dir.display());
    println!("default_parent: {}", config.default_parent);
    println!("managed_body_begin: {MANAGED_BODY_BEGIN}");
    println!("managed_body_end: {MANAGED_BODY_END}");
    println!(
        "pipeline_fields_excluded_from_marker_sync: {}",
        PIPELINE_FIELDS.join(",")
    );
}

fn print_phase_one_no_write_notice() {
    println!("phase: command skeleton");
    println!("writes: none");
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Sync Highlights PDF annotations into Bob reference notes")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            with_config_args(
                ClapCommand::new("scan")
                    .about("Scan the configured Highlights library")
                    .arg(dry_run_arg()),
            )
            .after_help("Phase 1 records the command surface only and does not modify vault files."),
        )
        .subcommand(
            with_config_args(
                ClapCommand::new("sync")
                    .about("Sync one PDF into its Bob reference note")
                    .arg(
                        Arg::new("pdf")
                            .value_name("PDF")
                            .required(true)
                            .value_parser(OsStringValueParser::new())
                            .help("PDF to sync"),
                    )
                    .arg(dry_run_arg())
                    .arg(
                        Arg::new("write-pdf")
                            .long("write-pdf")
                            .action(ArgAction::SetTrue)
                            .help("Allow future marker writes back to the PDF"),
                    )
                    .arg(
                        Arg::new("prefer")
                            .long("prefer")
                            .value_name("SIDE")
                            .value_parser(["marker", "frontmatter"])
                            .help("Future conflict override side"),
                    ),
            )
            .after_help("Phase 1 accepts the arguments but performs no PDF or vault writes."),
        )
        .subcommand(
            with_config_args(
                ClapCommand::new("doctor")
                    .about("Check Highlights reference sync prerequisites"),
            )
            .after_help("Phase 1 reports configuration only; prerequisite checks land later."),
        )
        .subcommand(
            with_config_args(
                ClapCommand::new("marker")
                    .about("Inspect the marker note contract for one PDF")
                    .arg(
                        Arg::new("pdf")
                            .value_name("PDF")
                            .required(true)
                            .value_parser(OsStringValueParser::new())
                            .help("PDF whose marker note should be inspected"),
                    ),
            )
            .after_help("The marker note is the first standalone PDF note annotation."),
        )
}

fn with_config_args(command: ClapCommand) -> ClapCommand {
    command
        .arg(
            Arg::new("bob-dir")
                .long("bob-dir")
                .value_name("PATH")
                .value_parser(OsStringValueParser::new())
                .help("Bob vault root; defaults to BOB_DIR or ~/bob"),
        )
        .arg(
            Arg::new("lib-dir")
                .long("lib-dir")
                .value_name("PATH")
                .value_parser(OsStringValueParser::new())
                .help("Highlights PDF library; defaults to BOB_HIGHLIGHTS_LIB_DIR or lib"),
        )
        .arg(
            Arg::new("ref-dir")
                .long("ref-dir")
                .value_name("PATH")
                .value_parser(OsStringValueParser::new())
                .help("Reference note output directory; defaults to BOB_HIGHLIGHTS_REF_DIR or ref"),
        )
        .arg(
            Arg::new("default-parent")
                .long("default-parent")
                .value_name("WIKILINK")
                .value_parser(OsStringValueParser::new())
                .help("Parent frontmatter fallback; defaults to BOB_HIGHLIGHTS_DEFAULT_PARENT or [[obsidian]]"),
        )
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .action(ArgAction::SetTrue)
        .help("Preview future work without modifying the vault or PDF")
}

impl Config {
    fn from_matches(matches: &ArgMatches) -> Self {
        let bob_dir = matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        let lib_dir = configured_path(
            matches,
            "lib-dir",
            ENV_LIB_DIR,
            DEFAULT_LIB_DIR,
            &bob_dir,
        );
        let ref_dir = configured_path(
            matches,
            "ref-dir",
            ENV_REF_DIR,
            DEFAULT_REF_DIR,
            &bob_dir,
        );
        let default_parent = configured_string(
            matches,
            "default-parent",
            ENV_DEFAULT_PARENT,
            DEFAULT_PARENT,
        );

        Self {
            bob_dir,
            lib_dir,
            ref_dir,
            default_parent,
        }
    }
}

fn configured_path(
    matches: &ArgMatches,
    arg_name: &str,
    env_name: &str,
    default_value: &str,
    bob_dir: &Path,
) -> PathBuf {
    let configured = matches
        .get_one::<OsString>(arg_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os(env_name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from(default_value));

    resolve_under_bob(bob_dir, &configured)
}

fn configured_string(
    matches: &ArgMatches,
    arg_name: &str,
    env_name: &str,
    default_value: &str,
) -> String {
    matches
        .get_one::<OsString>(arg_name)
        .filter(|value| !value.is_empty())
        .map(|value| os_to_string(value.as_os_str()))
        .or_else(|| env::var(env_name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| default_value.to_string())
}

fn resolve_under_bob(bob_dir: &Path, path: &Path) -> PathBuf {
    let expanded = bob_env::expand_tilde(path);
    if expanded.is_absolute() {
        expanded
    } else {
        bob_dir.join(expanded)
    }
}

fn required_path(matches: &ArgMatches, name: &str) -> PathBuf {
    let value = matches
        .get_one::<OsString>(name)
        .expect("required argument is enforced by clap");
    bob_env::expand_tilde(&PathBuf::from(value))
}

fn os_to_string(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn relative_config_paths_resolve_under_bob_dir() {
        let bob_dir = Path::new("/tmp/bob");

        assert_eq!(
            super::resolve_under_bob(bob_dir, Path::new("library")),
            PathBuf::from("/tmp/bob/library")
        );
        assert_eq!(
            super::resolve_under_bob(bob_dir, Path::new("/var/lib/pdfs")),
            PathBuf::from("/var/lib/pdfs")
        );
    }

    #[test]
    fn pipeline_fields_exclude_marker_user_projection() {
        assert!(super::PIPELINE_FIELDS.contains(&"source_pdf"));
        assert!(super::PIPELINE_FIELDS.contains(&"highlights_marker_hash"));
        assert!(!super::PIPELINE_FIELDS.contains(&"status"));
        assert!(!super::PIPELINE_FIELDS.contains(&"parent"));
    }
}
