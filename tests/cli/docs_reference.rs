//! Golden CLI reference: every catalog path's `--help` becomes
//! `docs/reference/cli/`, and the `cli nav` block in `mkdocs.yml`.
//!
//! The renderer derives every path from [`bob_cli::runner::docs_catalog`],
//! which itself derives from the same `SUBCOMMANDS` table as root help and
//! completion. A mismatch against the committed files — or a stray file in
//! that directory — fails with "CLI reference drifted; run `just docs-gen`".
//! `BOB_UPDATE_GOLDENS=1` writes the files instead of comparing.

use crate::support::*;
use bob_cli::runner::docs_catalog;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

const GOLDENS_ENV: &str = "BOB_UPDATE_GOLDENS";
const CLI_DIR: &str = "docs/reference/cli";
const GENERATOR: &str = "tests/cli/docs_reference.rs";
const MAX_DESCRIPTION: usize = 160;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run `bob <path> --help` hermetically and return its stdout.
fn capture_help(path: &[&str]) -> String {
    let mut command = bob_command();
    for part in path {
        command.arg(part);
    }
    let output = command.arg("--help").output().unwrap_or_else(|error| {
        panic!("run bob {} --help: {error}", path.join(" "))
    });
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        !help.as_bytes().contains(&0x1b),
        "bob {} --help must not emit ANSI escape codes:\n{}",
        path.join(" "),
        format_output(&output)
    );
    help
}

/// Every literal `docs/<stem>.md(#fragment)?` mention in help text.
fn docs_mentions(helps: &[&str]) -> BTreeSet<(String, Option<String>)> {
    let mut mentions = BTreeSet::new();
    for help in helps {
        let mut rest = *help;
        while let Some(start) = rest.find("docs/") {
            let after = &rest[start + "docs/".len()..];
            let end = after
                .bytes()
                .position(|byte| {
                    !(byte.is_ascii_alphanumeric()
                        || matches!(byte, b'_' | b'.' | b'/' | b'-' | b'#'))
                })
                .unwrap_or(after.len());
            let mut candidate: &str =
                after[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            candidate = candidate.trim_end_matches([')', '\'', '"', '`']);
            rest = &after[end..];
            let (page, fragment) = match candidate.split_once('#') {
                Some((page, fragment)) => (page, Some(fragment)),
                None => (candidate, None),
            };
            if !page.ends_with(".md")
                || page.starts_with('/')
                || page.contains("..")
                || page.len() <= ".md".len()
            {
                continue;
            }
            mentions.insert((
                page.to_owned(),
                fragment.filter(|part| !part.is_empty()).map(str::to_owned),
            ));
        }
    }
    mentions
}

/// First level-one heading of a committed docs page, falling back to the stem.
fn page_title(root: &std::path::Path, stem: &str) -> String {
    let path = root.join("docs").join(format!("{stem}.md"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return stem.to_owned();
    };
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return title.to_owned();
            }
        }
    }
    stem.to_owned()
}

fn truncate_description(about: &str) -> String {
    let flat = about.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() <= MAX_DESCRIPTION {
        return yaml_double_quoted(&flat);
    }
    let mut end = MAX_DESCRIPTION;
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = match flat[..end].rfind(' ') {
        Some(space) => flat[..space].to_owned(),
        None => flat[..end].to_owned(),
    };
    yaml_double_quoted(&truncated)
}

/// Quote one YAML front-matter scalar so colons never break the mapping.
fn yaml_double_quoted(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

fn fence(title: &str, body: &str) -> String {
    // The closing fence stands on its own line: a trailing fence infixed on
    // the content line never closes the block, and the strict build would
    // then scan the quoted help as prose.
    format!(
        "```text title=\"{title}\"\n{}\n```\n",
        body.trim_end_matches('\n')
    )
}

struct Member {
    name: String,
    about: String,
    help: String,
}

struct Command {
    name: String,
    about: String,
    help: String,
    members: Vec<Member>,
}

struct Section {
    title: String,
    commands: Vec<Command>,
}

fn flat_about(about: &str) -> String {
    about.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn related_section(
    root: &std::path::Path,
    helps: &[&str],
    depth: &str,
) -> String {
    let mentions = docs_mentions(helps);
    if mentions.is_empty() {
        return String::new();
    }
    let mut section = String::from("## Related\n\n");
    for (page, fragment) in &mentions {
        let stem = page.strip_suffix(".md").unwrap_or(page);
        let title = page_title(root, stem);
        let mut target = format!("{depth}{page}");
        if let Some(fragment) = fragment {
            target.push('#');
            target.push_str(fragment);
        }
        section.push_str(&format!("- [{title}]({target})\n"));
    }
    section.push('\n');
    section
}

fn render_command_page(root: &std::path::Path, command: &Command) -> String {
    let mut page = format!(
        "---\ntitle: bob {name}\ndescription: {description}\n---\n<!-- generated by {GENERATOR}; run just docs-gen -->\n\n# bob {name}\n\n{about}\n\n{fence}\n",
        name = command.name,
        about = command.about,
        description = truncate_description(&command.about),
        fence = fence(&format!("bob {} --help", command.name), &command.help),
    );
    for member in &command.members {
        page.push_str(&format!(
            "## bob {command} {member}\n\n{member_about}\n\n{fence}\n",
            command = command.name,
            member = member.name,
            member_about = member.about,
            fence = fence(
                &format!("bob {} {} --help", command.name, member.name),
                &member.help
            ),
        ));
    }
    let mut helps: Vec<&str> = vec![command.help.as_str()];
    helps.extend(command.members.iter().map(|member| member.help.as_str()));
    page.push_str(&related_section(root, &helps, "../../"));
    page.truncate(page.trim_end_matches('\n').len());
    page.push('\n');
    page
}

fn render_index(sections: &[Section], root_help: &str) -> String {
    let mut page = String::from(
        "---\ntitle: CLI reference\ndescription: Complete generated reference for every bob command and subcommand.\n---\n<!-- generated by tests/cli/docs_reference.rs; run just docs-gen -->\n\n# CLI reference\n\nEvery `bob` command, generated from the same help text `bob --help` prints.\n\n",
    );
    for section in sections {
        page.push_str(&format!(
            "## {title}\n\n| Command | What it does |\n| --- | --- |\n",
            title = section.title,
        ));
        for command in &section.commands {
            page.push_str(&format!(
                "| [`bob {name}`]({name}.md) | {about} |\n",
                name = command.name,
                about = command.about,
            ));
            for member in &command.members {
                let anchor = format!("bob-{}-{}", command.name, member.name)
                    .replace('_', "-");
                page.push_str(&format!(
                    "| [`bob {command} {member}`]({command}.md#{anchor}) | {about} |\n",
                    command = command.name,
                    member = member.name,
                    about = member.about,
                ));
            }
        }
        page.push('\n');
    }
    page.push_str(&format!(
        "## Full help\n\n{fence}",
        fence = fence("bob --help", root_help),
    ));
    page.truncate(page.trim_end_matches('\n').len());
    page.push('\n');
    page
}

fn render_cli_nav(sections: &[Section]) -> String {
    let mut block = String::new();
    for section in sections {
        block.push_str(&format!(
            "          - \"{title}\":\n",
            title = section.title
        ));
        for command in &section.commands {
            block.push_str(&format!(
                "              - \"bob {name}\": reference/cli/{name}.md\n",
                name = command.name,
            ));
        }
    }
    block
}

fn replace_region(text: &str, name: &str, content: &str) -> String {
    let begin = format!("# BEGIN GENERATED: {name}");
    let end = format!("# END GENERATED: {name}");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == begin)
        .unwrap_or_else(|| panic!("mkdocs.yml has no {begin} marker"));
    let finish = lines
        .iter()
        .position(|line| line.trim() == end)
        .unwrap_or_else(|| panic!("mkdocs.yml has no {end} marker"));
    assert!(
        finish > start,
        "generated region {name} markers are out of order"
    );
    let mut out = lines[..=start].join("\n");
    out.push('\n');
    out.push_str(content.trim_end_matches('\n'));
    out.push('\n');
    out.push_str(&lines[finish..].join("\n"));
    out.push('\n');
    out
}

#[test]
fn cli_reference_is_current() {
    let root = repo_root();
    let catalog = docs_catalog();
    assert!(!catalog.is_empty(), "docs catalog must not be empty");

    let root_help = capture_help(&[]);

    // Paths in catalog order: every top-level command, then its members.
    let mut sections: Vec<Section> = Vec::new();
    for section in &catalog {
        let mut commands = Vec::new();
        for command in &section.commands {
            let mut members = Vec::new();
            for member in &command.members {
                members.push(Member {
                    name: member.name.to_owned(),
                    about: flat_about(member.about),
                    help: capture_help(&[command.name, member.name]),
                });
            }
            commands.push(Command {
                name: command.name.to_owned(),
                about: flat_about(command.about),
                help: capture_help(&[command.name]),
                members,
            });
        }
        sections.push(Section {
            title: section.title.to_owned(),
            commands,
        });
    }

    // The visible root alias resolves to its canonical member help.
    let alias_help = capture_help(&["notify"]);
    let canonical = sections
        .iter()
        .flat_map(|section| section.commands.iter())
        .find(|command| command.name == "pomodoro")
        .and_then(|command| {
            command
                .members
                .iter()
                .find(|member| member.name == "notify")
        })
        .expect("catalog covers pomodoro notify");
    assert_eq!(
        alias_help, canonical.help,
        "`bob notify --help` must match `bob pomodoro notify --help`"
    );

    let mut desired: BTreeMap<PathBuf, String> = BTreeMap::new();
    for section in &sections {
        for command in &section.commands {
            let page = render_command_page(&root, command);
            desired.insert(
                root.join(CLI_DIR).join(format!("{}.md", command.name)),
                page,
            );
        }
    }
    desired.insert(
        root.join(CLI_DIR).join("index.md"),
        render_index(&sections, &root_help),
    );

    let config_path = root.join("mkdocs.yml");
    let config = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|error| panic!("read mkdocs.yml: {error}"));
    let desired_config =
        replace_region(&config, "cli nav", &render_cli_nav(&sections));

    let update = std::env::var(GOLDENS_ENV).as_deref() == Ok("1");
    if update {
        for (path, content) in &desired {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap_or_else(|error| {
                    panic!("create {}: {error}", parent.display())
                });
            }
            std::fs::write(path, content).unwrap_or_else(|error| {
                panic!("write {}: {error}", path.display())
            });
            println!("generated: {}", path.display());
        }
        if config != desired_config {
            std::fs::write(&config_path, &desired_config)
                .unwrap_or_else(|error| panic!("write mkdocs.yml: {error}"));
            println!("generated: mkdocs.yml (cli nav)");
        }
        return;
    }

    let mut drifted = false;
    for (path, content) in &desired {
        match std::fs::read_to_string(path) {
            Ok(current) if &current == content => {}
            _ => {
                println!("out of date: {}", path.display());
                drifted = true;
            }
        }
    }
    if config != desired_config {
        println!("out of date: mkdocs.yml (cli nav)");
        drifted = true;
    }
    // A stray file in the CLI reference directory is drift, too.
    let cli_dir = root.join(CLI_DIR);
    if let Ok(entries) = std::fs::read_dir(&cli_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md")
                && !desired.contains_key(&path)
            {
                println!("stray file: {}", path.display());
                drifted = true;
            }
        }
    }
    assert!(!drifted, "CLI reference drifted; run `just docs-gen`");
}
