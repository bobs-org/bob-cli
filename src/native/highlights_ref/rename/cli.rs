use clap::{
    builder::OsStringValueParser, Arg, ArgAction, Command as ClapCommand,
};

use super::plan;
use crate::native::highlights_ref::{bob_dir_arg, ref_dir_arg, xlib_dir_arg};

pub(crate) fn command() -> ClapCommand {
    ClapCommand::new("rename")
        .about("Rename a reference's files and update every link to it")
        .arg(
            Arg::new("ref")
                .value_name("REF")
                .required(true)
                .value_parser(clap::value_parser!(String))
                .help("Reference: note path, stem, id, URL, arXiv ID, DOI, or exact title"),
        )
        .arg(
            Arg::new("name")
                .value_name("NAME")
                .required(true)
                .value_parser(clap::value_parser!(String))
                .help("New file stem; a trailing .pdf is ignored"),
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Print the plan without moving, rewriting, committing, or syncing"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .help("Output format [default: human] [possible values: human, json]"),
        )
        .arg(
            Arg::new("lib-dir")
                .long("lib-dir")
                .short('l')
                .value_name("PATH")
                .value_parser(OsStringValueParser::new())
                .help("Highlights PDF library; defaults to BOB_HIGHLIGHTS_LIB_DIR or lib"),
        )
        .arg(
            Arg::new("offline")
                .long("offline")
                .short('o')
                .action(ArgAction::SetTrue)
                .help("Commit locally; skip vault sync before and after"),
        )
        .arg(ref_dir_arg())
        .arg(xlib_dir_arg())
        .after_help(
            "The frontmatter id and reading task's ^ref block ID stay, so Today, Task Links, dependencies, and bob ref find <old id> keep working. v1 refuses annotated, legacy, and intake references. Links outside the vault, including browser bookmarks and copied obsidian:// or highlights:// URIs, cannot be updated.\n\nExamples:\n  bob ref rename 442a6bf555914893e9891c11519de94f karpathy_llm_wiki -d\n  bob ref rename ref/chat/README.md research_prototype_cdx",
        )
}

pub(crate) fn run(matches: &clap::ArgMatches) -> i32 {
    plan::run(matches)
}
