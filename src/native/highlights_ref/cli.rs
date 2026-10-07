//! CLI construction and config reporting.
use std::{
    env,
    io::{self, IsTerminal},
};

use super::*;

/// Grouped subcommand listing: the single source of `bob ref` help groups.
///
/// Each entry is a group title plus the subcommand names it owns, in display
/// order. Phase `find` adds the Library group alongside the pipeline group.
pub(crate) const HELP_GROUPS: &[(&str, &[&str])] = &[(
    "Highlights pipeline",
    &["clip", "create", "doctor", "marker", "scan", "sync"],
)];

const REF_HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}

{before-help}Options:
{options}{after-help}";

const REF_AFTER_HELP: &str = "\
Examples:
  bob ref create <URL|PDF|MD> -L   Capture a reference and narrate it
  bob ref scan                     Sync Highlights PDFs into reference notes
";

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
    let subcommands = all_subcommands();
    let groups = render_help_groups(&subcommands, help_groups_color());
    let mut command = ClapCommand::new(COMMAND_NAME)
        .about(
            "Find, list, and read Bob reference notes, and sync Highlights PDFs into them",
        )
        .help_template(REF_HELP_TEMPLATE)
        .before_help(groups)
        .after_help(REF_AFTER_HELP)
        .disable_help_subcommand(true)
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(no_hooks_arg());
    for subcommand in subcommands {
        command = command.subcommand(subcommand);
    }
    command
}

/// Every `bob ref` subcommand builder, in `HELP_GROUPS` order.
fn all_subcommands() -> Vec<ClapCommand> {
    vec![
        clip::command(),
        create::command(),
        doctor_command(),
        marker_command(),
        scan_command(),
        sync_command(),
    ]
}

fn doctor_command() -> ClapCommand {
    ClapCommand::new("doctor")
        .about("Check Highlights reference sync prerequisites")
        .arg(bob_dir_arg())
        .arg(lib_dir_arg())
        .arg(no_hooks_arg())
        .arg(ref_dir_arg())
        .arg(xlib_dir_arg())
        .after_help(
            "Checks vault paths, sidecars, PDF markers, Git state, and optional ob support.",
        )
}

fn marker_command() -> ClapCommand {
    with_config_args(
        ClapCommand::new("marker")
            .about("Inspect the marker note for one PDF")
            .arg(pdf_arg("PDF whose marker note should be inspected")),
    )
    .after_help(
        "The marker note is the first standalone /Text annotation on page 1.",
    )
}

fn scan_command() -> ClapCommand {
    with_scan_args(
        ClapCommand::new("scan")
            .about("Scan the configured Highlights library"),
    )
    .after_help(
        "Examples:\n  bob ref scan --dry-run\n\nScans PDFs recursively, preflights collisions and dirty targets, then syncs each PDF.",
    )
}

fn sync_command() -> ClapCommand {
    with_sync_args(ClapCommand::new("sync"))
}

/// Render the `HELP_GROUPS` listing shown between usage and options.
///
/// Rows follow the root help style: a cyan literal name when color is on,
/// wrapped at 80 columns.
fn render_help_groups(subcommands: &[ClapCommand], color: bool) -> String {
    let mut out = String::new();
    for (index, (title, names)) in HELP_GROUPS.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(title);
        out.push_str(":\n");
        let width = names.iter().map(|name| name.len()).max().unwrap_or(0);
        for name in *names {
            let about = subcommands
                .iter()
                .find(|command| command.get_name() == *name)
                .and_then(|command| command.get_about())
                .map(|about| about.to_string())
                .unwrap_or_default();
            append_help_group_row(&mut out, name, &about, width, color);
        }
    }
    out
}

fn append_help_group_row(
    out: &mut String,
    name: &str,
    about: &str,
    width: usize,
    color: bool,
) {
    let prefix_len = 2 + width + 2;
    let available = 80usize.saturating_sub(prefix_len);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in about.split_whitespace() {
        let next_width = line.chars().count()
            + usize::from(!line.is_empty())
            + word.chars().count();
        if next_width > available && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    out.push_str("  ");
    if color {
        out.push_str("\u{1b}[1;36m");
    }
    out.push_str(name);
    if color {
        out.push_str("\u{1b}[0m");
    }
    out.push_str(&" ".repeat(width - name.len() + 2));
    out.push_str(&lines[0]);
    out.push('\n');
    let continuation = " ".repeat(prefix_len);
    for line in lines.iter().skip(1) {
        out.push_str(&continuation);
        out.push_str(line);
        out.push('\n');
    }
}

fn help_groups_color() -> bool {
    io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_groups_cover_every_subcommand_exactly_once() {
        let mounted: Vec<String> = build_cli()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .collect();
        let grouped: Vec<String> = HELP_GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().map(|name| name.to_string()))
            .collect();
        assert!(!mounted.is_empty(), "expected mounted subcommands",);
        let mut mounted_sorted = mounted.clone();
        mounted_sorted.sort();
        let mut grouped_sorted = grouped.clone();
        grouped_sorted.sort();
        assert_eq!(
            mounted_sorted, grouped_sorted,
            "every subcommand belongs in exactly one help group",
        );
    }

    #[test]
    fn help_groups_are_alphabetical_with_matching_abouts() {
        let subcommands = all_subcommands();
        for (title, names) in HELP_GROUPS {
            let mut sorted = names.to_vec();
            sorted.sort_unstable();
            assert_eq!(
                names.to_vec(),
                sorted,
                "group `{title}` is not alphabetical",
            );
            for name in *names {
                let about = subcommands
                    .iter()
                    .find(|command| command.get_name() == *name)
                    .and_then(|command| command.get_about())
                    .map(|about| about.to_string())
                    .unwrap_or_default();
                assert!(
                    !about.is_empty(),
                    "group `{title}` member `{name}` has no about",
                );
                let rendered = render_help_groups(&subcommands, false);
                assert!(
                    rendered.contains(name) && rendered.contains(&about),
                    "group `{title}` member `{name}` about is not rendered",
                );
            }
        }
    }
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
        .help("Reference note directory; defaults to BOB_HIGHLIGHTS_REF_DIR or ref")
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
