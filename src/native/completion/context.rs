//! Partial-parse context for vault-aware completion slots.
//!
//! The engine resolves each cursor position to a slot, but clap's
//! `ValueCompleter` sees only the current word. This module recovers
//! the values typed before the cursor (`--bob-dir`, `--route`,
//! `--task`, `--repo`, …) with a tolerant walk over one composed
//! [`tree`](super::tree), so short clusters (`-rcash`),
//! `--route=cash`, and environment defaults behave exactly as at
//! runtime. Providers receive an immutable [`Context`]; completion
//! never writes.

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use clap::{Arg, Command as ClapCommand};

use super::tree;
use crate::native::{capture, env as bob_env};

/// Option values typed before the cursor, resolved the way the
/// runtime resolves them.
#[derive(Debug, Clone)]
pub(crate) struct Context {
    /// Subcommand path, e.g. `["capture"]`.
    pub path: Vec<String>,
    /// `--bob-dir`, else [`bob_env::bob_dir`], which honors `BOB_DIR`.
    pub bob_dir: PathBuf,
    /// `--route` / `-r`, lowercased like the runtime normalizes it.
    pub route: Option<String>,
    /// `--task` / `-t` (a parent block ID).
    pub task: Option<String>,
    /// `--repo` / `-r` on `plugins sync`, else [`bob_env::plugins_dir`].
    pub repo: Option<PathBuf>,
}

impl Context {
    /// Parse the words before the cursor (the command word excluded,
    /// like the presenter's walk). Never fails: unknown words are
    /// skipped so one typo cannot blank every vault slot.
    pub(crate) fn parse(before: &[OsString]) -> Self {
        // `build` fills in each arg's `num_args` defaults; without it
        // every option looks valueless and no values are recorded.
        let before = crate::runner::rewrite_alias_args(before);
        let mut root = tree();
        root.build();
        let mut walker = Walker::new(&root);
        for word in &before {
            walker.step(word);
        }
        walker.finish()
    }
}

struct Walker<'a> {
    node: &'a ClapCommand,
    path: Vec<String>,
    pending: Option<String>,
    escaped: bool,
    bob_dir: Option<OsString>,
    route: Option<String>,
    task: Option<String>,
    repo: Option<OsString>,
}

impl<'a> Walker<'a> {
    fn new(root: &'a ClapCommand) -> Self {
        Self {
            node: root,
            path: Vec::new(),
            pending: None,
            escaped: false,
            bob_dir: None,
            route: None,
            task: None,
            repo: None,
        }
    }

    fn step(&mut self, word: &OsString) {
        let text = word.to_string_lossy();
        if self.escaped {
            return;
        }
        // A pending option consumes any non-flag word as its value.
        if let Some(id) = self.pending.take()
            && !(text.starts_with('-') && text != "-")
        {
            self.record(&id, &text);
            return;
        }
        self.pending = None;
        if text == "--" {
            self.escaped = true;
            return;
        }
        if let Some(stripped) = text.strip_prefix("--") {
            let (name, inline) = match stripped.split_once('=') {
                Some((name, inline)) => (name, Some(inline)),
                None => (stripped, None),
            };
            if let Some(arg) = find_long(self.node, name) {
                let id = arg.get_id().to_string();
                match inline {
                    Some(value) => self.record(&id, value),
                    None if takes_values(arg) => {
                        self.pending = Some(id);
                    }
                    None => {}
                }
            }
            return;
        }
        if text.starts_with('-') && text != "-" {
            let chars: Vec<char> = text[1..].chars().collect();
            let mut index = 0;
            while index < chars.len() {
                let Some(arg) = find_short(self.node, chars[index]) else {
                    break;
                };
                let id = arg.get_id().to_string();
                if takes_values(arg) {
                    let rest: String = chars[index + 1..].iter().collect();
                    match rest.strip_prefix('=').unwrap_or(&rest) {
                        "" => self.pending = Some(id),
                        value => self.record(&id, value),
                    }
                    break;
                }
                index += 1;
            }
            return;
        }
        if let Some(next) = self.node.find_subcommand(word) {
            self.node = next;
            self.path.push(next.get_name().to_string());
        }
    }

    fn record(&mut self, id: &str, value: &str) {
        match id {
            "bob-dir" => self.bob_dir = Some(OsString::from(value)),
            "route" => self.route = Some(normalize_route(value)),
            "task" | "task-ref" | "block-id"
                if id == "task" || self.task.is_none() =>
            {
                self.task = Some(value.to_string());
            }
            "repo" => self.repo = Some(OsString::from(value)),
            _ => {}
        }
    }

    fn finish(self) -> Context {
        Context {
            path: self.path,
            bob_dir: self
                .bob_dir
                .map(PathBuf::from)
                .map(|path| bob_env::expand_tilde(&path))
                .unwrap_or_else(bob_env::bob_dir),
            route: self.route.filter(|route| !route.is_empty()),
            task: self.task.filter(|task| !task.is_empty()),
            repo: Some(
                self.repo
                    .map(PathBuf::from)
                    .map(|path| bob_env::expand_tilde(&path))
                    .unwrap_or_else(bob_env::plugins_dir),
            ),
        }
    }
}

/// The runtime lowercases valid route tokens; anything else fails
/// later, and providers then complete an empty set.
fn normalize_route(route: &str) -> String {
    if capture::is_route_token(route) {
        route.to_ascii_lowercase()
    } else {
        route.to_string()
    }
}

fn find_long<'b>(command: &'b ClapCommand, name: &str) -> Option<&'b Arg> {
    command.get_arguments().find(|arg| {
        arg.get_long() == Some(name)
            || arg
                .get_visible_aliases()
                .is_some_and(|aliases| aliases.contains(&name))
    })
}

fn find_short(command: &ClapCommand, short: char) -> Option<&Arg> {
    command.get_arguments().find(|arg| {
        arg.get_short() == Some(short)
            || arg
                .get_visible_short_aliases()
                .is_some_and(|aliases| aliases.contains(&short))
    })
}

fn takes_values(arg: &Arg) -> bool {
    arg.get_num_args().is_some_and(|range| range.takes_values())
}

/// Split an attached cursor word (`--route=cash`) for tests.
#[allow(dead_code)]
pub(crate) fn split_attached(cursor: &OsStr) -> Option<(String, String)> {
    let text = cursor.to_string_lossy();
    let stripped = text.strip_prefix("--")?;
    let (name, value) = stripped.split_once('=')?;
    Some((name.to_string(), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(words: &[&str]) -> Context {
        Context::parse(&words.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn parses_long_route_and_bob_dir() {
        let context =
            context(&["capture", "--bob-dir", "/tmp/vault", "--route", "Cash"]);
        assert_eq!(context.path, vec!["capture".to_string()]);
        assert_eq!(context.bob_dir, PathBuf::from("/tmp/vault"));
        assert_eq!(context.route.as_deref(), Some("cash"));
    }

    #[test]
    fn parses_attached_and_short_cluster_forms() {
        let attached = context(&["capture", "--route=cash"]);
        assert_eq!(attached.route.as_deref(), Some("cash"));

        let cluster = context(&["capture", "-rcash"]);
        assert_eq!(cluster.route.as_deref(), Some("cash"));

        let short_value = context(&["capture-sections", "-r", "dev"]);
        assert_eq!(short_value.route.as_deref(), Some("dev"));
        assert_eq!(short_value.path, vec!["capture-sections".to_string()]);
    }

    #[test]
    fn task_and_repo_need_their_subcommands() {
        let task = context(&["capture", "--route", "cash", "--task", "abc"]);
        assert_eq!(task.task.as_deref(), Some("abc"));

        let block_id =
            context(&["capture-task-sections", "-r", "cash", "-i", "abc"]);
        assert_eq!(block_id.task.as_deref(), Some("abc"));

        let repo = context(&["plugins", "sync", "--repo", "/tmp/repo"]);
        assert_eq!(repo.repo, Some(PathBuf::from("/tmp/repo")));
    }

    #[test]
    fn alias_words_rewrite_to_the_canonical_path() {
        let context = context(&["randomize", "--level", "P2"]);
        assert_eq!(
            context.path,
            vec!["task".to_string(), "reroll".to_string()]
        );
    }

    #[test]
    fn unknown_words_never_fail_the_parse() {
        let context = context(&["capture", "--bogus", "x", "--route", "cash"]);
        assert_eq!(context.route.as_deref(), Some("cash"));
    }

    #[test]
    fn env_defaults_apply_without_flags() {
        let context = context(&["capture"]);
        assert_eq!(context.bob_dir, bob_env::bob_dir());
        assert_eq!(context.route, None);
        assert_eq!(context.repo, Some(bob_env::plugins_dir()));
    }
}
