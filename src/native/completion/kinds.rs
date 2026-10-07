//! The static value-kinds table: `(command path, arg id)` → [`Kind`].
//!
//! Every value-taking argument in [`tree`](super::tree) has a decision:
//! clap possible values ([`Kind::Choices`], served live by the engine),
//! a path directive ([`Kind::Dirs`] / [`Kind::Files`]), free text
//! ([`Kind::FreeText`], served as a `!message` hint), or a vault slot
//! with a live read-only provider ([`Kind::Route`] and friends, served
//! by [`providers`](super::providers)).
//!
//! The key needs the command path because one arg id can mean two
//! things: `--block-id` is a _new_ ID in `capture-task-id` but an
//! _existing_ task in `capture-task-sections`, `--source` is a choice
//! in `gkeep list` but free text in `query`, and `--output` is a PDF in
//! `ref create` but whatever its
//! [`clap::ValueHint`] says elsewhere. An empty path matches any
//! command; a full-path entry wins over the all-commands entry, and a
//! non-trivial [`clap::ValueHint`] sits between the two (see the
//! presenter's `value_lines`). The coverage tests below keep the table
//! exhaustive over the tree in both directions.

/// How to complete the value of one argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Clap possible values; the engine serves them live.
    Choices,
    /// Complete directories natively (`!dirs`).
    Dirs,
    /// Complete files natively (`!files`, optionallly filtered by glob).
    Files(Option<&'static str>),
    /// Free text; the presenter shows a `!message` hint.
    FreeText,
    /// A vault route from the capture-targets scan.
    Route,
    /// A section of the routed note.
    Section,
    /// An open task of the routed note, by block ID.
    Task,
    /// An ALL-CAPS child section of the `--task` parent.
    TaskSection,
    /// An open Pomodoro, by stale-safe ref.
    PomodoroRef,
    /// A plugin ID from the repo checkout.
    Plugin,
    /// A configured `randomize` priority label.
    Level,
    /// A vault note, completed as a path relative to the vault root.
    VaultNote,
}

struct Entry {
    /// Command path, e.g. `&["gkeep", "pull"]`. Empty matches anywhere.
    path: &'static [&'static str],
    /// Clap argument id.
    arg: &'static str,
    kind: Kind,
}

/// Static decisions, most specific entry first per (path, arg).
const TABLE: &[Entry] = &[
    // One id, two meanings: a _new_ ID here ...
    Entry {
        path: &["capture-task-id"],
        arg: "block-id",
        kind: Kind::FreeText,
    },
    // ... but an _existing_ task here.
    Entry {
        path: &["capture-task-sections"],
        arg: "block-id",
        kind: Kind::Task,
    },
    // A choice here ...
    Entry {
        path: &["gkeep", "list"],
        arg: "source",
        kind: Kind::Choices,
    },
    // ... but free text here.
    Entry {
        path: &["query"],
        arg: "source",
        kind: Kind::FreeText,
    },
    // Directories.
    Entry {
        path: &[],
        arg: "bob-dir",
        kind: Kind::Dirs,
    },
    Entry {
        path: &[],
        arg: "repo",
        kind: Kind::Dirs,
    },
    Entry {
        path: &[],
        arg: "backup-dir",
        kind: Kind::Dirs,
    },
    Entry {
        path: &[],
        arg: "lib-dir",
        kind: Kind::Dirs,
    },
    Entry {
        path: &[],
        arg: "ref-dir",
        kind: Kind::Dirs,
    },
    Entry {
        path: &[],
        arg: "xlib-dir",
        kind: Kind::Dirs,
    },
    Entry {
        path: &[],
        arg: "vault",
        kind: Kind::Dirs,
    },
    // Files.
    Entry {
        path: &[],
        arg: "query-file",
        kind: Kind::Files(None),
    },
    Entry {
        path: &[],
        arg: "tasks-file",
        kind: Kind::Files(None),
    },
    Entry {
        path: &[],
        arg: "target",
        kind: Kind::Files(Some("*.md *.pdf")),
    },
    Entry {
        path: &[],
        arg: "html",
        kind: Kind::Files(Some("*.html")),
    },
    Entry {
        path: &[],
        arg: "pdf",
        kind: Kind::Files(Some("*.pdf")),
    },
    // `--output` is a PDF only on the ref command whose builder
    // says so; everywhere else it falls through to its `ValueHint` (or
    // to free text when it has none).
    Entry {
        path: &["ref", "create"],
        arg: "output",
        kind: Kind::Files(Some("*.pdf")),
    },
    // `bob ref find` takes free-text queries and a score floor.
    Entry {
        path: &["ref", "find"],
        arg: "query",
        kind: Kind::FreeText,
    },
    Entry {
        path: &["ref", "find"],
        arg: "min-score",
        kind: Kind::FreeText,
    },
    // `bob ref list` overrides the global `origin` (a vault note
    // everywhere else) with a static choice, and adds its own choice
    // and free-text slots.
    Entry {
        path: &["ref", "list"],
        arg: "origin",
        kind: Kind::Choices,
    },
    Entry {
        path: &["ref", "list"],
        arg: "reading-state",
        kind: Kind::Choices,
    },
    Entry {
        path: &["ref", "list"],
        arg: "since",
        kind: Kind::FreeText,
    },
    // `bob ref show` takes vault-note references.
    Entry {
        path: &["ref", "show"],
        arg: "ref",
        kind: Kind::VaultNote,
    },
    // Static choices (also served live by the engine through clap
    // possible values; the entries keep the decision explicit).
    Entry {
        path: &[],
        arg: "format",
        kind: Kind::Choices,
    },
    Entry {
        path: &[],
        arg: "engine",
        kind: Kind::Choices,
    },
    Entry {
        path: &[],
        arg: "prefer",
        kind: Kind::Choices,
    },
    Entry {
        path: &[],
        arg: "status",
        kind: Kind::Choices,
    },
    // Vault slots with live read-only providers. Stale-safe refs
    // (`--task-ref`) stay free text: they name one task, not a set.
    // `ref --parent` is a bare note name, also free text.
    Entry {
        path: &[],
        arg: "route",
        kind: Kind::Route,
    },
    Entry {
        path: &[],
        arg: "section",
        kind: Kind::Section,
    },
    Entry {
        path: &[],
        arg: "task",
        kind: Kind::Task,
    },
    Entry {
        path: &[],
        arg: "task-ref",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "task-section",
        kind: Kind::TaskSection,
    },
    Entry {
        path: &[],
        arg: "pomodoro-ref",
        kind: Kind::PomodoroRef,
    },
    Entry {
        path: &[],
        arg: "plugin",
        kind: Kind::Plugin,
    },
    Entry {
        path: &[],
        arg: "level",
        kind: Kind::Level,
    },
    Entry {
        path: &[],
        arg: "tasks-note",
        kind: Kind::VaultNote,
    },
    Entry {
        path: &[],
        arg: "origin",
        kind: Kind::VaultNote,
    },
    Entry {
        path: &[],
        arg: "note",
        kind: Kind::VaultNote,
    },
    Entry {
        path: &[],
        arg: "note-path",
        kind: Kind::VaultNote,
    },
    Entry {
        path: &[],
        arg: "parent",
        kind: Kind::FreeText,
    },
    // Capture TEXT is free text when reached as a plain value slot;
    // the live marker extraction serves it first.
    Entry {
        path: &[],
        arg: "text",
        kind: Kind::FreeText,
    },
    // Free text: everything else.
    Entry {
        path: &[],
        arg: "author",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "cap",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "clip",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "cursor",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "email",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "id",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "jobs",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "limit",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "message",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "name",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "published",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "query",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "ref-type",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "retry-timeout",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "seed",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "tasks",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "threshold",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "title",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "until",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "PRE_CHECK_SLEEP",
        kind: Kind::FreeText,
    },
    Entry {
        path: &[],
        arg: "POST_NOTIFY_SLEEP",
        kind: Kind::FreeText,
    },
];

/// The full-path entry for `(command path, arg id)`, if any. A
/// non-trivial [`clap::ValueHint`] sits between this and
/// [`lookup_generic`]; see the presenter's `value_lines`.
pub(crate) fn lookup_exact(path: &[&str], arg: &str) -> Option<Kind> {
    TABLE
        .iter()
        .find(|entry| entry.arg == arg && entry.path == path)
        .map(|entry| entry.kind)
}

/// The all-commands entry for one arg id, if any.
pub(crate) fn lookup_generic(arg: &str) -> Option<Kind> {
    TABLE
        .iter()
        .find(|entry| entry.arg == arg && entry.path.is_empty())
        .map(|entry| entry.kind)
}

/// Descriptions for static choice values, used when the clap
/// `PossibleValue` carries no help of its own. They add information
/// and never repeat the value.
pub(crate) fn choice_description(
    arg: &str,
    value: &str,
) -> Option<&'static str> {
    match (arg, value) {
        ("format", "human") => Some("Colored text for people"),
        ("format", "json") => Some("Machine-readable JSON"),
        ("format", "table") => Some("Human-readable table"),
        ("format", "markdown") => Some("Rendered Markdown"),
        ("format", "paths") => Some("Matching note paths"),
        ("engine", "native") => Some("Dataview and Tasks"),
        ("engine", "obsidian") => Some("Live Dataview"),
        ("source", "both") => Some("Both inboxes"),
        ("source", "keep") => Some("Keep only"),
        ("source", "vault") => Some("The vault only"),
        ("prefer", "marker") => Some("Use the marker side"),
        ("prefer", "frontmatter") => Some("Use the frontmatter side"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::native::completion::tree;

    /// Every (command path, arg id) in the tree that takes a value.
    fn value_args(
        command: &clap::Command,
        path: &[String],
    ) -> Vec<(Vec<String>, String)> {
        let mut found = Vec::new();
        for arg in command.get_arguments() {
            let takes_values =
                arg.get_num_args().is_some_and(|range| range.takes_values());
            if takes_values {
                found.push((path.to_vec(), arg.get_id().to_string()));
            }
        }
        for subcommand in command.get_subcommands() {
            let mut child = path.to_vec();
            child.push(subcommand.get_name().to_string());
            found.extend(value_args(subcommand, &child));
        }
        found
    }

    #[test]
    fn every_value_arg_has_a_decision() {
        // `build` fills in each arg's `num_args` defaults; unbuilt
        // args report none and the walk below would pass vacuously.
        let mut built = tree();
        built.build();
        let root: &clap::Command = &built;
        let missing: Vec<String> = value_args(root, &[])
            .into_iter()
            .filter(|(path, id)| {
                let short: Vec<&str> =
                    path.iter().map(String::as_str).collect();
                if lookup_exact(&short, id)
                    .or_else(|| lookup_generic(id))
                    .is_some()
                {
                    return false;
                }
                // Possible values and non-trivial value hints decide
                // the slot without a table entry.
                let command = short.iter().fold(root, |command, name| {
                    command
                        .find_subcommand(name)
                        .expect("coverage walk follows the tree")
                });
                let arg = command
                    .get_arguments()
                    .find(|arg| arg.get_id() == id)
                    .expect("coverage walk follows the tree");
                if !arg.get_possible_values().is_empty() {
                    return false;
                }
                if !matches!(
                    arg.get_value_hint(),
                    clap::ValueHint::Unknown | clap::ValueHint::Other
                ) {
                    return false;
                }
                true
            })
            .map(|(path, id)| {
                if path.is_empty() {
                    id
                } else {
                    format!("{}:{id}", path.join(" "))
                }
            })
            .collect();
        assert!(
            missing.is_empty(),
            "value-taking args without a kinds decision: {missing:?}"
        );
    }

    #[test]
    fn every_table_entry_matches_the_tree() {
        let mut built = tree();
        built.build();
        let root: &clap::Command = &built;
        let mut problems = Vec::new();
        let mut seen = HashSet::new();
        for entry in TABLE {
            let key = (entry.path.join(" "), entry.arg);
            if !seen.insert(key.clone()) {
                problems.push(format!(
                    "duplicate table entry for {}:{}",
                    key.0, key.1
                ));
                continue;
            }
            if entry.path.is_empty() {
                let mut matches = 0;
                let mut stack = vec![root];
                while let Some(command) = stack.pop() {
                    if command
                        .get_arguments()
                        .any(|arg| arg.get_id() == entry.arg)
                    {
                        matches += 1;
                    }
                    stack.extend(command.get_subcommands());
                }
                if matches == 0 {
                    problems.push(format!(
                        "all-commands entry matches no arg: {}",
                        entry.arg
                    ));
                }
                continue;
            }
            let mut command = root;
            let mut ok = true;
            for name in entry.path {
                match command.find_subcommand(name) {
                    Some(next) => command = next,
                    None => {
                        problems.push(format!(
                            "entry path missing from tree: {}",
                            entry.path.join(" ")
                        ));
                        ok = false;
                        break;
                    }
                }
            }
            if ok
                && !command.get_arguments().any(|arg| arg.get_id() == entry.arg)
            {
                problems.push(format!(
                    "entry arg missing from {}: {}",
                    entry.path.join(" "),
                    entry.arg
                ));
            }
        }
        assert!(problems.is_empty(), "stale kinds entries: {problems:?}");
    }

    #[test]
    fn path_specific_entries_win() {
        assert_eq!(
            lookup_exact(&["capture-task-id"], "block-id"),
            Some(Kind::FreeText)
        );
        // `capture-task-sections --block-id` names an existing
        // task, so the vault-kinds phase gives it the live task
        // provider; `capture-task-id --block-id` stays free text
        // because it mints a new ID.
        assert_eq!(
            lookup_exact(&["capture-task-sections"], "block-id"),
            Some(Kind::Task)
        );
        assert_eq!(
            lookup_exact(&["gkeep", "list"], "source"),
            Some(Kind::Choices)
        );
        // `--output` is a PDF only on the ref command whose
        // builder says so; the generic lookup finds no entry.
        assert_eq!(
            lookup_exact(&["ref", "create"], "output"),
            Some(Kind::Files(Some("*.pdf")))
        );
        assert_eq!(lookup_exact(&["completion", "zsh"], "output"), None);
        assert_eq!(lookup_generic("output"), None);
        assert_eq!(lookup_exact(&["query"], "source"), Some(Kind::FreeText));
        assert_eq!(lookup_generic("source"), None);
        assert_eq!(lookup_generic("tasks"), Some(Kind::FreeText));
        assert_eq!(lookup_generic("bob-dir"), Some(Kind::Dirs));
        assert_eq!(lookup_generic("text"), Some(Kind::FreeText));
    }
}
