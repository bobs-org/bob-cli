//! Assemble the catalog document from the static table and the clap tree.

use std::collections::{BTreeMap, BTreeSet};

use clap::Command as ClapCommand;

use super::table::{
    ExitHint, Row, StaticArg, CAPTURE_PROTOCOL_REASON, COMPLETION_REASON,
    ENTRIES, EXCLUDED,
};
use crate::native::completion::tree;
use crate::runner::{subcommands, Section, ALIASES, SECTIONS};

#[derive(Debug, Clone)]
pub(crate) struct Catalog {
    pub schema_version: u32,
    pub ok: bool,
    pub protocol: u32,
    pub bob_version: String,
    pub features: Vec<String>,
    pub sections: Vec<CatalogSection>,
    pub roots: Vec<String>,
    pub aliases: Vec<Alias>,
    pub entries: Vec<Entry>,
    pub excluded: Vec<ExcludedPath>,
}

#[derive(Debug, Clone)]
pub(crate) struct CatalogSection {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Alias {
    pub from: Vec<String>,
    pub to: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub id: String,
    pub path: Vec<String>,
    pub fixed: Vec<String>,
    pub title: String,
    pub about: String,
    pub section: String,
    pub keywords: Vec<String>,
    pub effect: String,
    pub network: bool,
    pub presentation: String,
    pub policy: String,
    pub preview: Option<Vec<String>>,
    pub confirm: Option<String>,
    pub budget_secs: u32,
    pub args: Vec<Arg>,
    pub exit_codes: Vec<ExitCode>,
    pub adapter: Option<String>,
    pub hosts: Vec<String>,
    pub invalidates: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Arg {
    pub name: String,
    pub slot: String,
    pub required: bool,
    pub repeat: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ExitCode {
    pub code: i32,
    pub outcome: String,
    pub meaning: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExcludedPath {
    pub path: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PublicLeaf {
    pub path: Vec<String>,
    pub about: String,
    pub positionals: Vec<Positional>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub flags: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Positional {
    pub id: String,
    pub name: String,
    pub required: bool,
    pub repeat: bool,
}

/// Build the protocol-1 catalog. Never reads the vault, config, or caches.
pub(crate) fn document() -> Catalog {
    let leaves = public_leaves();
    let by_path = leaf_index(&leaves);
    Catalog {
        schema_version: 1,
        ok: true,
        protocol: 1,
        bob_version: env!("CARGO_PKG_VERSION").to_string(),
        features: Vec::new(),
        sections: catalog_sections(),
        roots: catalog_roots(),
        aliases: catalog_aliases(),
        entries: ENTRIES
            .iter()
            .map(|row| assemble_entry(row, &by_path))
            .collect(),
        excluded: catalog_excluded(&leaves),
    }
}

pub(crate) fn public_leaves() -> Vec<PublicLeaf> {
    let mut root = tree();
    root.build();
    let mut leaves = Vec::new();
    walk_leaves(&root, Vec::new(), &mut leaves);
    leaves
}

fn walk_leaves(
    command: &ClapCommand,
    path: Vec<String>,
    out: &mut Vec<PublicLeaf>,
) {
    let visible: Vec<&ClapCommand> = command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
        .collect();
    if visible.is_empty() {
        if !path.is_empty() {
            out.push(leaf_from(command, path));
        }
        return;
    }
    for sub in visible {
        let mut child = path.clone();
        child.push(sub.get_name().to_string());
        walk_leaves(sub, child, out);
    }
}

fn leaf_from(command: &ClapCommand, path: Vec<String>) -> PublicLeaf {
    let about = command
        .get_about()
        .map(|about| about.to_string())
        .unwrap_or_default();
    let about = about.lines().next().unwrap_or("").trim().to_string();
    let mut positionals = Vec::new();
    for arg in command.get_positionals() {
        if arg.is_hide_set() {
            continue;
        }
        let id = arg.get_id().as_str().to_string();
        if id == "help" || id == "version" {
            continue;
        }
        let name = arg
            .get_value_names()
            .and_then(|names| names.first())
            .map(|name| name.to_string())
            .unwrap_or_else(|| id.to_ascii_uppercase());
        positionals.push(Positional {
            id,
            name,
            required: arg.is_required_set(),
            repeat: positional_repeats(arg),
        });
    }
    PublicLeaf {
        path,
        about,
        positionals,
        flags: flag_tokens(command),
    }
}

fn positional_repeats(arg: &clap::Arg) -> bool {
    if matches!(arg.get_action(), clap::ArgAction::Append) {
        return true;
    }
    arg.get_num_args()
        .is_some_and(|range| range.max_values() > 1)
}

fn flag_tokens(command: &ClapCommand) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();
    for arg in command.get_arguments() {
        if arg.is_positional() {
            continue;
        }
        if let Some(long) = arg.get_long() {
            flags.insert(format!("--{long}"));
        }
        if let Some(short) = arg.get_short() {
            flags.insert(format!("-{short}"));
        }
        if let Some(aliases) = arg.get_all_aliases() {
            for alias in aliases {
                flags.insert(format!("--{alias}"));
            }
        }
        if let Some(aliases) = arg.get_all_short_aliases() {
            for alias in aliases {
                flags.insert(format!("-{alias}"));
            }
        }
    }
    flags
}

fn leaf_index(leaves: &[PublicLeaf]) -> BTreeMap<Vec<String>, &PublicLeaf> {
    leaves
        .iter()
        .map(|leaf| (leaf.path.clone(), leaf))
        .collect()
}

fn catalog_sections() -> Vec<CatalogSection> {
    SECTIONS
        .iter()
        .filter_map(|section| {
            section.catalog_id().map(|id| CatalogSection {
                id: id.to_string(),
                title: section.title().to_string(),
            })
        })
        .collect()
}

fn catalog_roots() -> Vec<String> {
    let mut roots = BTreeSet::new();
    for entry in subcommands() {
        if entry.section.catalog_id().is_some() {
            roots.insert(entry.name.to_string());
        }
    }
    for alias in ALIASES {
        roots.insert(alias.from.to_string());
    }
    roots.into_iter().collect()
}

fn catalog_aliases() -> Vec<Alias> {
    let mut aliases = Vec::new();
    for alias in ALIASES {
        aliases.push(Alias {
            from: vec![alias.from.to_string()],
            to: alias.to.iter().map(|part| part.to_string()).collect(),
        });
    }
    let mut root = tree();
    root.build();
    walk_aliases(&root, Vec::new(), &mut aliases);
    aliases.sort_by(|left, right| left.from.cmp(&right.from));
    aliases.dedup_by(|left, right| left.from == right.from);
    aliases
}

fn walk_aliases(
    command: &ClapCommand,
    path: Vec<String>,
    out: &mut Vec<Alias>,
) {
    if !path.is_empty() {
        for alias in command.get_all_aliases() {
            let mut from = path.clone();
            if let Some(last) = from.last_mut() {
                *last = alias.to_string();
            }
            out.push(Alias {
                from,
                to: path.clone(),
            });
        }
    }
    for sub in command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
    {
        let mut child = path.clone();
        child.push(sub.get_name().to_string());
        walk_aliases(sub, child, out);
    }
}

fn assemble_entry(
    row: &Row,
    by_path: &BTreeMap<Vec<String>, &PublicLeaf>,
) -> Entry {
    let path: Vec<String> =
        row.path.iter().map(|part| part.to_string()).collect();
    let leaf = by_path.get(&path);
    let about = leaf.map(|leaf| leaf.about.clone()).unwrap_or_default();
    let mut args = Vec::new();
    if let Some(leaf) = leaf {
        for positional in &leaf.positionals {
            args.push(Arg {
                name: positional.name.clone(),
                slot: slot_for(row, &positional.id, &positional.name),
                required: positional.required,
                repeat: positional.repeat,
            });
        }
    }
    for extra in row.extra_args {
        if args
            .iter()
            .any(|arg| arg.name.eq_ignore_ascii_case(extra.name))
        {
            continue;
        }
        args.push(arg_from_static(extra));
    }
    let section = section_id(row.path).unwrap_or("vault").to_string();
    Entry {
        id: row.id.to_string(),
        path,
        fixed: row.fixed.iter().map(|part| part.to_string()).collect(),
        title: row.title.to_string(),
        about,
        section,
        keywords: row.keywords.iter().map(|word| word.to_string()).collect(),
        effect: row.effect.as_str().to_string(),
        network: row.network,
        presentation: row.presentation.as_str().to_string(),
        policy: row.policy.as_str().to_string(),
        preview: row
            .preview
            .map(|parts| parts.iter().map(|part| part.to_string()).collect()),
        confirm: row.confirm.map(str::to_string),
        budget_secs: row.budget_secs,
        args,
        exit_codes: row.exit_codes.iter().map(exit_from_static).collect(),
        adapter: row.adapter.map(str::to_string),
        hosts: row.hosts.iter().map(|host| host.to_string()).collect(),
        invalidates: row
            .invalidates
            .iter()
            .map(|key| key.to_string())
            .collect(),
    }
}

fn arg_from_static(arg: &StaticArg) -> Arg {
    Arg {
        name: arg.name.to_string(),
        slot: arg.slot.to_string(),
        required: arg.required,
        repeat: arg.repeat,
    }
}

fn exit_from_static(hint: &ExitHint) -> ExitCode {
    ExitCode {
        code: hint.code,
        outcome: hint.outcome.to_string(),
        meaning: hint.meaning.to_string(),
    }
}

fn slot_for(row: &Row, id: &str, name: &str) -> String {
    for (key, slot) in row.slots {
        if key.eq_ignore_ascii_case(id) || key.eq_ignore_ascii_case(name) {
            return slot.to_string();
        }
    }
    "text".to_string()
}

fn section_id(path: &[&str]) -> Option<&'static str> {
    let root = path.first()?;
    subcommands()
        .iter()
        .find(|entry| entry.name == *root)
        .and_then(|entry| entry.section.catalog_id())
}

fn catalog_excluded(leaves: &[PublicLeaf]) -> Vec<ExcludedPath> {
    let mut excluded: Vec<ExcludedPath> = EXCLUDED
        .iter()
        .map(|row| ExcludedPath {
            path: row.path.iter().map(|part| part.to_string()).collect(),
            reason: row.reason.to_string(),
        })
        .collect();
    let mut seen: BTreeSet<Vec<String>> =
        excluded.iter().map(|row| row.path.clone()).collect();
    for leaf in leaves {
        let Some(root) = leaf.path.first() else {
            continue;
        };
        let section = root_section(root);
        let reason = if section == Some(Section::CaptureProtocol) {
            Some(CAPTURE_PROTOCOL_REASON)
        } else if root == "completion" {
            Some(COMPLETION_REASON)
        } else {
            None
        };
        let Some(reason) = reason else {
            continue;
        };
        if seen.insert(leaf.path.clone()) {
            excluded.push(ExcludedPath {
                path: leaf.path.clone(),
                reason: reason.to_string(),
            });
        }
    }
    excluded.sort_by(|left, right| left.path.cmp(&right.path));
    excluded
}

fn root_section(name: &str) -> Option<Section> {
    subcommands()
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.section)
}

/// Tokens in `preview` / `fixed` that must exist as flags on the leaf.
#[cfg(test)]
pub(crate) fn option_tokens(parts: &[&str]) -> Vec<String> {
    parts
        .iter()
        .filter_map(|part| {
            if *part == "--" || *part == "-" {
                None
            } else if let Some(long) = part.strip_prefix("--") {
                let name =
                    long.split_once('=').map(|(name, _)| name).unwrap_or(long);
                Some(format!("--{name}"))
            } else if part.starts_with('-') && part.chars().count() == 2 {
                Some((*part).to_string())
            } else {
                None
            }
        })
        .collect()
}
