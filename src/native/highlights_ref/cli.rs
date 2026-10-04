//! CLI construction and config reporting.
use super::*;

pub(super) fn print_config_report(operation: &str, config: &Config) {
    println!("Highlights reference sync");
    println!("operation: {operation}");
    println!("bob_dir: {}", config.bob_dir.display());
    println!("lib_dir: {}", config.lib_dir.display());
    println!("ref_dir: {}", config.ref_dir.display());
    println!("xlib_dir: {}", config.xlib_dir.display());
    println!("managed_body_begin: {MANAGED_BODY_BEGIN}");
    println!("managed_body_end: {MANAGED_BODY_END}");
    println!(
        "pipeline_fields_excluded_from_marker_sync: {}",
        PIPELINE_FIELDS.join(",")
    );
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Sync Highlights PDF annotations into Bob reference notes")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(no_hooks_arg())
        .subcommand(clip::command())
        .subcommand(create::command())
        .subcommand(
            ClapCommand::new("doctor")
                .about("Check Highlights reference sync prerequisites")
                .arg(bob_dir_arg())
                .arg(lib_dir_arg())
                .arg(no_hooks_arg())
                .arg(ref_dir_arg())
                .arg(xlib_dir_arg())
            .after_help("Checks vault paths, sidecars, PDF markers, Git state, and optional ob support."),
        )
        .subcommand(
            with_config_args(
                ClapCommand::new("marker")
                    .about("Inspect the marker note for one PDF")
                    .arg(pdf_arg("PDF whose marker note should be inspected")),
            )
            .after_help("The marker note is the first standalone /Text annotation on page 1."),
        )
        .subcommand(
            with_scan_args(
                ClapCommand::new("scan")
                    .about("Scan the configured Highlights library"),
            )
            .after_help("Examples:\n  bob highlights scan --dry-run\n\nScans PDFs recursively, preflights collisions and dirty targets, then syncs each PDF."),
        )
        .subcommand(with_sync_args(ClapCommand::new("sync")))
}

pub(super) fn with_config_args(command: ClapCommand) -> ClapCommand {
    command
        .arg(bob_dir_arg())
        .arg(lib_dir_arg())
        .arg(ref_dir_arg())
        .arg(xlib_dir_arg())
}

pub(super) fn with_scan_args(command: ClapCommand) -> ClapCommand {
    command
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(jobs_arg())
        .arg(lib_dir_arg())
        .arg(no_hooks_arg())
        .arg(ref_dir_arg())
        .arg(verbose_arg())
        .arg(write_pdfs_arg())
        .arg(xlib_dir_arg())
}

pub(super) fn no_hooks_arg() -> Arg {
    Arg::new("no-hooks")
        .long("no-hooks")
        .short('n')
        .action(ArgAction::SetTrue)
        .help("Ignore the configured highlights.pre_scan_hook")
}

pub(super) fn with_sync_args(command: ClapCommand) -> ClapCommand {
    command
        .about("Sync one PDF marker note into its Bob reference note")
        .arg(pdf_arg("PDF to sync"))
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(lib_dir_arg())
        .arg(prefer_arg())
        .arg(ref_dir_arg())
        .arg(write_pdf_arg())
        .arg(xlib_dir_arg())
        .after_help("The first standalone /Text annotation on page 1 is treated as the marker note.")
}

pub(super) fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

pub(super) fn lib_dir_arg() -> Arg {
    Arg::new("lib-dir")
        .long("lib-dir")
        .short('l')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help(
            "Highlights PDF library; defaults to BOB_HIGHLIGHTS_LIB_DIR or lib",
        )
}

pub(super) fn ref_dir_arg() -> Arg {
    Arg::new("ref-dir")
        .long("ref-dir")
        .short('r')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Reference note output directory; defaults to BOB_HIGHLIGHTS_REF_DIR or ref")
}

pub(super) fn xlib_dir_arg() -> Arg {
    Arg::new("xlib-dir")
        .long("xlib-dir")
        .short('x')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Highlights PDF intake directory; defaults to BOB_HIGHLIGHTS_XLIB_DIR or xlib")
}

pub(super) fn pdf_arg(help: &'static str) -> Arg {
    Arg::new("pdf")
        .value_name("PDF")
        .required(true)
        .value_parser(OsStringValueParser::new())
        .help(help)
}

pub(super) fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview work without modifying the vault or PDF")
}

pub(super) fn jobs_arg() -> Arg {
    Arg::new("jobs")
        .long("jobs")
        .short('j')
        .value_name("N")
        .value_parser(clap::value_parser!(u64).range(1..))
        .help(
            "Process this many PDFs in parallel; defaults to available CPU cores (use 1 to force sequential)",
        )
}

pub(super) fn prefer_arg() -> Arg {
    Arg::new("prefer")
        .long("prefer")
        .short('p')
        .value_name("SIDE")
        .value_parser(["marker", "frontmatter"])
        .help("Resolve a marker/frontmatter conflict using this side")
}

pub(super) fn verbose_arg() -> Arg {
    Arg::new("verbose")
        .long("verbose")
        .short('v')
        .action(ArgAction::SetTrue)
        .help("Print the detailed per-PDF scan report")
}

pub(super) fn write_pdf_arg() -> Arg {
    Arg::new("write-pdf")
        .long("write-pdf")
        .short('w')
        .action(ArgAction::SetTrue)
        .help("Allow marker writes back to the PDF")
}

pub(super) fn write_pdfs_arg() -> Arg {
    Arg::new("write-pdfs")
        .long("write-pdfs")
        .short('w')
        .action(ArgAction::SetTrue)
        .help("Allow marker writes back to all PDFs during scan")
}
