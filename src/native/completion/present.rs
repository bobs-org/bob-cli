//! Post-filter and presenter for `__complete` requests.
//!
//! The engine resolves each cursor position to a slot from a
//! slot-normalized current word (empty for values and commands, `-` or
//! `--` for options, `--opt=` with an empty value for attached values),
//! so upstream prefix filtering never narrows a slot and the shell keeps
//! filtering. This module then applies the presentation rules:
//!
//! 1. An empty cursor word offers subcommands and positional values,
//!    and options only when nothing else applies.
//! 2. A lone `-` offers short and long forms adjacent with identical
//!    descriptions; a `--` word gets long forms only.
//! 3. Options already present (unless `Append`/`Count`), options that
//!    conflict with present ones, and `-h/--help`/`-V/--version` once
//!    other arguments exist are dropped.
//! 4. Nothing is re-sorted: subcommands follow `SUBCOMMANDS` order,
//!    options follow help order.
//! 5. Descriptions add information and never repeat the value.
//! 6. Group headers are human words (`commands`, `capture protocol`,
//!    `options`, per-slot value groups).
//! 7. Once a trailing var-arg `TEXT` has started, or after `--`, no
//!    options or subcommands are offered.

use std::ffi::{OsStr, OsString};

use clap::{Arg, ArgAction, Command as ClapCommand};

use super::engine;
use super::kinds::{self, Kind};
use super::protocol::{self, Request};
use super::tree;
use super::{context, providers};
use crate::runner::{subcommands, CompletionTier};

/// Answer one parsed request with protocol 1 response lines.
pub(crate) fn complete_request(request: &Request) -> Vec<String> {
    let mut root = tree();
    root.build();
    let words = &request.words;
    // Words after the ignored command word, excluding the cursor word.
    let before: &[OsString] = if words.len() > 2 {
        &words[1..words.len() - 1]
    } else {
        &[]
    };
    let cursor: &OsStr =
        words.last().map(OsString::as_os_str).unwrap_or_default();

    let mut walk = Walk::new(&root, before);
    for (index, word) in before.iter().enumerate() {
        walk.step(word, index);
    }
    walk.finish(cursor, request.suffix.as_ref())
}

/// Partial-parse context: the subcommand path, the options seen, and
/// whether the value/text tail has started.
struct Walk<'a> {
    node: &'a ClapCommand,
    /// Words after the ignored command word, excluding the cursor word.
    before: &'a [OsString],
    path: Vec<String>,
    /// Arg ids seen before the cursor, first-seen order.
    present: Vec<String>,
    /// Arg id expecting its value in the next word, if any.
    pending: Option<String>,
    /// A `--` separator was seen.
    escaped: bool,
    /// A trailing var-arg positional has started.
    text_seen: bool,
    /// Index in `before` where TEXT began, for `raw_text` reconstruction.
    text_start: Option<usize>,
    /// Before-words consumed by subcommand descent.
    path_words: usize,
    /// Plain words after the subcommand path consumed as positional
    /// fills (option values, flags, and TEXT never count).
    positional_words: usize,
}

impl<'a> Walk<'a> {
    fn new(root: &'a ClapCommand, before: &'a [OsString]) -> Self {
        Self {
            node: root,
            before,
            path: Vec::new(),
            present: Vec::new(),
            pending: None,
            escaped: false,
            text_seen: false,
            text_start: None,
            path_words: 0,
            positional_words: 0,
        }
    }

    fn push_present(&mut self, id: &str) {
        if !self.present.iter().any(|seen| seen == id) {
            self.present.push(id.to_string());
        }
    }

    fn mark_text_started(&mut self, index: usize) {
        self.text_seen = true;
        if self.text_start.is_none() {
            self.text_start = Some(index);
        }
    }

    fn step(&mut self, word: &OsString, index: usize) {
        let text = word.to_string_lossy();
        if self.escaped {
            if has_trailing_text(self.node) {
                self.mark_text_started(index);
            } else {
                self.positional_words += 1;
            }
            return;
        }
        // A pending option consumes any non-flag word as its value. A
        // bare `-` reads as stdin for file-ish options, matching clap.
        if self.pending.is_some() && !(text.starts_with('-') && text != "-") {
            self.pending = None;
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
                self.push_present(&id);
                if takes_values(arg) && inline.is_none() {
                    self.pending = Some(id);
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
                self.push_present(&id);
                if takes_values(arg) {
                    let rest: String = chars[index + 1..].iter().collect();
                    if rest.strip_prefix('=').unwrap_or(&rest).is_empty() {
                        self.pending = Some(id);
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
            self.path_words = index + 1;
            self.positional_words = 0;
            return;
        }
        if has_trailing_text(self.node) {
            self.mark_text_started(index);
        } else {
            self.positional_words += 1;
        }
    }

    fn is_capture_text_command(&self) -> bool {
        matches!(
            self.path.first().map(String::as_str),
            Some("capture" | "capture-parse" | "capture-rewrite")
        )
    }

    /// TEXT slot for the capture trio through the in-process
    /// `capture_complete` extraction. Returns `None` when no marker applies
    /// so the caller can fall back (empty cursor) or show nothing.
    fn capture_text_opt(
        &self,
        cursor: &OsStr,
        suffix: Option<&OsString>,
    ) -> Option<Vec<String>> {
        super::capture_text::capture_text_lines(
            self.before,
            self.text_start,
            cursor,
            suffix,
        )
    }

    fn finish(&self, cursor: &OsStr, suffix: Option<&OsString>) -> Vec<String> {
        let cursor_text = cursor.to_string_lossy();
        // Attached `--opt=value`: the option is resolved, the value part
        // is normalized away, and candidates replace only the rest.
        if let Some(stripped) = cursor_text.strip_prefix("--")
            && let Some((name, _)) = stripped.split_once('=')
            && !name.is_empty()
            && let Some(arg) = find_long(self.node, name)
            && takes_values(arg)
        {
            let prefix = format!("--{name}=");
            return self.value_lines(arg, Some(&prefix));
        }
        if cursor_text.starts_with('-') {
            if self.escaped || self.text_seen {
                if self.is_capture_text_command() {
                    // TEXT has started: never fall back to options, even
                    // when the word holds no marker (`bob capture fix --r`).
                    return self
                        .capture_text_opt(cursor, suffix)
                        .unwrap_or_default();
                }
                return self.text_lines();
            }
            return self.option_lines(self.node, cursor_text.starts_with("--"));
        }
        if let Some(id) = &self.pending
            && let Some(arg) = find_by_id(self.node, id)
        {
            return self.value_lines(arg, None);
        }
        if self.escaped || self.text_seen {
            if self.is_capture_text_command() {
                return self
                    .capture_text_opt(cursor, suffix)
                    .unwrap_or_default();
            }
            return self.text_lines();
        }
        // The cursor word is TEXT when it sits at the TEXT position and does
        // not start with `-` (the `-` case returned above).
        if self.is_capture_text_command() && has_trailing_text(self.node) {
            match self.capture_text_opt(cursor, suffix) {
                Some(lines) => return lines,
                None if cursor_text.is_empty() => {}
                // A non-empty first TEXT word with no marker (`bob capture
                // fix<TAB> as one word): TEXT has started with the cursor,
                // so no options.
                None => return Vec::new(),
            }
        }
        self.commands_or_value_lines()
    }

    /// Options for one command: rule 2 pairing, rule 3 filtering, rule 4
    /// help order, rule 6 `options` group.
    fn option_lines(
        &self,
        node: &ClapCommand,
        longs_only: bool,
    ) -> Vec<String> {
        let others_exist = self.other_args_exist();
        // Per-arg groups in first-seen (help) order; shorts before longs
        // so each pair stays adjacent with identical descriptions.
        let mut groups: Vec<(usize, Vec<super::engine::Candidate>)> =
            Vec::new();
        for candidate in engine::option_candidates(node) {
            if longs_only
                && !candidate.value.to_string_lossy().starts_with("--")
            {
                continue;
            }
            match groups
                .iter_mut()
                .find(|(_, members)| members[0].id == candidate.id)
            {
                Some((_, members)) => members.push(candidate),
                None => groups.push((candidate.order, vec![candidate])),
            }
        }
        groups.sort_by_key(|(order, _)| *order);
        let mut lines = Vec::new();
        for (_, mut members) in groups {
            members.sort_by_key(|member| {
                member.value.to_string_lossy().starts_with("--")
            });
            for member in members {
                let Some(arg) = member
                    .id
                    .as_deref()
                    .and_then(|id| id.strip_prefix("arg::"))
                    .and_then(|id| find_by_id(node, id))
                else {
                    continue;
                };
                if arg.is_hide_set() {
                    continue;
                }
                let id = arg.get_id().to_string();
                if self.present.iter().any(|seen| seen == &id)
                    && !matches!(
                        arg.get_action(),
                        ArgAction::Append | ArgAction::Count
                    )
                {
                    continue;
                }
                if self.conflicts_with_present(node, arg) {
                    continue;
                }
                if others_exist && (id == "help" || id == "version") {
                    continue;
                }
                let help = member.help.as_deref().unwrap_or_default();
                if let Some(line) = protocol::candidate_line(
                    &member.value,
                    help,
                    "options",
                    false,
                ) {
                    lines.push(line);
                }
            }
        }
        lines
    }

    /// Next unfilled positional of the current node, if any. Words
    /// consumed as option values never count; a trailing var-arg TEXT
    /// is returned as-is so the caller can keep its current behavior.
    fn next_positional(&self) -> Option<Arg> {
        let mut remaining = self.positional_words;
        for arg in self.node.get_positionals() {
            if arg.is_trailing_var_arg_set() {
                return Some(arg.clone());
            }
            if remaining == 0 {
                return Some(arg.clone());
            }
            remaining -= 1;
        }
        None
    }

    /// Subcommands and positional values for a plain cursor word (rule
    /// 1); options only when nothing else applies.
    fn commands_or_value_lines(&self) -> Vec<String> {
        let mut engine_command = tree();
        let args: Vec<OsString> = std::iter::once(OsString::from("bob"))
            .chain(self.before_words())
            .chain(std::iter::once(OsString::new()))
            .collect();
        let index = args.len() - 1;
        let candidates = engine::complete(&mut engine_command, args, index);
        let mut subs: Vec<engine::Candidate> = Vec::new();
        let mut values: Vec<engine::Candidate> = Vec::new();
        for candidate in candidates {
            if candidate.hidden {
                continue;
            }
            let Some(id) = candidate.id.as_deref() else {
                continue;
            };
            if id.starts_with("command::") {
                let name = candidate.value.to_string_lossy().into_owned();
                if name == "help" {
                    continue;
                }
                subs.push(candidate);
            } else if !candidate.value.to_string_lossy().starts_with('-') {
                values.push(candidate);
            }
        }
        if subs.is_empty() && values.is_empty() {
            // Rule 1 for positional value slots: an empty cursor word at
            // a positional with a directive or value decision offers that
            // slot through the same `value_lines` path. Free-text
            // positionals and capture TEXT keep the options fallback.
            if let Some(positional) = self.next_positional()
                && !positional.is_trailing_var_arg_set()
            {
                let path: Vec<&str> =
                    self.path.iter().map(String::as_str).collect();
                if has_value_decision(&path, &positional) {
                    return self.value_lines(&positional, None);
                }
            }
            return self.option_lines(self.node, false);
        }
        let mut lines = Vec::new();
        if self.path.is_empty() {
            // Rule 4: SUBCOMMANDS order; rule 6: porcelain first under
            // `commands`, the ten frontend endpoints under
            // `capture protocol` last.
            for tier in [CompletionTier::Porcelain, CompletionTier::Plumbing] {
                let group = if tier == CompletionTier::Porcelain {
                    "commands"
                } else {
                    "capture protocol"
                };
                for entry in subcommands() {
                    if entry.tier != tier {
                        continue;
                    }
                    let Some(candidate) = subs.iter().find(|candidate| {
                        candidate.value.to_string_lossy() == entry.name
                    }) else {
                        continue;
                    };
                    let help = candidate.help.as_deref().unwrap_or_default();
                    if let Some(line) = protocol::candidate_line(
                        &candidate.value,
                        help,
                        group,
                        false,
                    ) {
                        lines.push(line);
                    }
                }
            }
        } else {
            for candidate in &subs {
                let help = candidate.help.as_deref().unwrap_or_default();
                if let Some(line) = protocol::candidate_line(
                    &candidate.value,
                    help,
                    "commands",
                    false,
                ) {
                    lines.push(line);
                }
            }
        }
        // Positional value candidates (possible-values positionals);
        // none exist in protocol 1, but the slot is handled generally.
        for candidate in &values {
            let Some(arg) = candidate
                .id
                .as_deref()
                .and_then(|id| id.strip_prefix("arg::"))
                .and_then(|id| find_by_id(self.node, id))
            else {
                continue;
            };
            let group = value_group(arg);
            let help = candidate.help.as_deref().unwrap_or_default();
            let help = if help.is_empty() {
                kinds::choice_description(
                    arg.get_id().as_ref(),
                    &candidate.value.to_string_lossy(),
                )
                .unwrap_or_default()
            } else {
                help
            };
            if let Some(line) =
                protocol::candidate_line(&candidate.value, help, &group, false)
            {
                lines.push(line);
            }
        }
        lines
    }

    /// The value slot for one option: engine choices or kinds lines.
    /// `attached` carries the `--opt=` prefix for `!prefix` replies.
    ///
    /// Order of precedence: a path-specific kinds entry beats a
    /// `ValueHint`; a `ValueHint` other than `Unknown` / `Other` beats
    /// a generic kinds entry and the free-text fallback.
    fn value_lines(&self, arg: &Arg, attached: Option<&str>) -> Vec<String> {
        let id = arg.get_id().to_string();
        let path: Vec<&str> = self.path.iter().map(String::as_str).collect();
        if !arg.get_possible_values().is_empty() {
            return self.choice_lines(arg, attached);
        }
        if let Some(kind) = kinds::lookup_exact(&path, &id) {
            return self.kind_lines(kind, arg, attached);
        }
        if let Some(line) = hint_line(arg) {
            return self.directive_lines(attached, &[line]);
        }
        match kinds::lookup_generic(&id) {
            Some(kind) => self.kind_lines(kind, arg, attached),
            None => {
                let message = free_text_message(arg);
                self.directive_lines(
                    attached,
                    &[protocol::message_line(&message)],
                )
            }
        }
    }

    /// One kinds decision as response lines: engine choices, vault
    /// slots, path directives, or the free-text message.
    fn kind_lines(
        &self,
        kind: Kind,
        arg: &Arg,
        attached: Option<&str>,
    ) -> Vec<String> {
        match kind {
            Kind::Choices => self.choice_lines(arg, attached),
            Kind::Route
            | Kind::Section
            | Kind::Task
            | Kind::TaskSection
            | Kind::PomodoroRef
            | Kind::Plugin
            | Kind::Level
            | Kind::VaultNote => {
                // Vault slots read the words before the cursor for
                // `--bob-dir`, `--route`, `--task`, and `--repo`, so
                // short clusters and `--opt=value` behave as at
                // runtime.
                let slot = context::Context::parse(self.before);
                debug_assert_eq!(
                    slot.path, self.path,
                    "completion context disagrees with presenter walk"
                );
                match providers::vault_lines(kind, &slot) {
                    Some(lines) => self.directive_lines(attached, &lines),
                    None => Vec::new(),
                }
            }
            Kind::Dirs => {
                self.directive_lines(attached, &[protocol::dirs_line()])
            }
            Kind::Files(glob) => {
                self.directive_lines(attached, &[protocol::files_line(glob)])
            }
            Kind::FreeText => {
                let message = free_text_message(arg);
                self.directive_lines(
                    attached,
                    &[protocol::message_line(&message)],
                )
            }
        }
    }

    /// Live possible values from the engine, slot-normalized.
    fn choice_lines(&self, arg: &Arg, attached: Option<&str>) -> Vec<String> {
        let id = arg.get_id().to_string();
        let normalized = attached.map(OsString::from).unwrap_or_default();
        let mut engine_command = tree();
        let args: Vec<OsString> = std::iter::once(OsString::from("bob"))
            .chain(self.before_words())
            .chain(std::iter::once(normalized))
            .collect();
        let index = args.len() - 1;
        let group = value_group(arg);
        let mut lines = Vec::new();
        for candidate in engine::complete(&mut engine_command, args, index) {
            if candidate.hidden {
                continue;
            }
            let mut value = candidate.value.to_string_lossy().into_owned();
            if let Some(prefix) = attached {
                let Some(stripped) = value.strip_prefix(prefix) else {
                    continue;
                };
                value = stripped.to_string();
            }
            let help = candidate.help.as_deref();
            let help =
                help.filter(|help| !help.is_empty()).unwrap_or_else(|| {
                    kinds::choice_description(&id, &value).unwrap_or_default()
                });
            if let Some(line) = protocol::candidate_line(
                OsStr::new(&value),
                help,
                &group,
                false,
            ) {
                lines.push(line);
            }
        }
        self.directive_lines(attached, &lines)
    }

    /// `!prefix` goes first when the cursor word keeps a prefix, then
    /// the body lines.
    fn directive_lines(
        &self,
        attached: Option<&str>,
        body: &[String],
    ) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(prefix) = attached {
            lines.push(protocol::prefix_line(prefix.chars().count()));
        }
        lines.extend(body.iter().cloned());
        lines
    }

    /// Rule 7 slots: no options or subcommands, only the positional
    /// message (or directives), or nothing when no positional exists.
    fn text_lines(&self) -> Vec<String> {
        let positional = self
            .node
            .get_positionals()
            .find(|arg| arg.is_trailing_var_arg_set())
            .or_else(|| self.node.get_positionals().next());
        match positional {
            Some(arg) => {
                let owned: Arg = arg.clone();
                self.value_lines(&owned, None)
            }
            None => Vec::new(),
        }
    }

    fn before_words(&self) -> impl Iterator<Item = OsString> + '_ {
        self.before.iter().cloned()
    }

    /// Whether any words exist after the subcommand path: options,
    /// values, or positional text beyond the command itself.
    fn other_args_exist(&self) -> bool {
        self.before.len() > self.path_words
    }

    /// Whether `candidate` is unusable next to the present options,
    /// checked in both directions.
    fn conflicts_with_present(
        &self,
        node: &ClapCommand,
        candidate: &Arg,
    ) -> bool {
        let candidate_id = candidate.get_id().to_string();
        // `get_arg_conflicts_with` panics when a conflict target is
        // unknown to the command; the tree's debug asserts guard that,
        // and a future skew must not break completion.
        let conflicts_of = |arg: &Arg| -> Vec<String> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                node.get_arg_conflicts_with(arg)
            }))
            .unwrap_or_default()
            .iter()
            .map(|arg| arg.get_id().to_string())
            .collect()
        };
        if conflicts_of(candidate)
            .iter()
            .any(|id| self.present.iter().any(|seen| seen == id))
        {
            return true;
        }
        self.present.iter().any(|seen| {
            find_by_id(node, seen).is_some_and(|seen_arg| {
                conflicts_of(seen_arg).iter().any(|id| id == &candidate_id)
            })
        })
    }
}

/// Elision stays ambiguous with two input borrows, so the lifetimes stay.
#[allow(clippy::needless_lifetimes)]
fn find_long<'a>(command: &'a ClapCommand, name: &str) -> Option<&'a Arg> {
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

/// Elision stays ambiguous with two input borrows, so the lifetimes stay.
#[allow(clippy::needless_lifetimes)]
fn find_by_id<'a>(command: &'a ClapCommand, id: &str) -> Option<&'a Arg> {
    command.get_arguments().find(|arg| arg.get_id() == id)
}

fn takes_values(arg: &Arg) -> bool {
    arg.get_num_args().is_some_and(|range| range.takes_values())
}

fn has_trailing_text(command: &ClapCommand) -> bool {
    command
        .get_positionals()
        .any(|arg| arg.is_trailing_var_arg_set())
}

/// The value group is named after the slot: the lowercased value name.
fn value_group(arg: &Arg) -> String {
    arg.get_value_names()
        .and_then(|names| names.first())
        .map(|name| name.to_string().to_lowercase())
        .unwrap_or_else(|| "values".to_string())
}

/// A non-trivial `ValueHint` as one native directive line, if any.
fn hint_line(arg: &Arg) -> Option<String> {
    match arg.get_value_hint() {
        clap::ValueHint::DirPath => Some(protocol::dirs_line()),
        clap::ValueHint::FilePath
        | clap::ValueHint::AnyPath
        | clap::ValueHint::ExecutablePath => Some(protocol::files_line(None)),
        _ => None,
    }
}

/// Whether one value slot offers directives or value rows (files,
/// dirs, vault notes, choices, or a `ValueHint`) rather than a
/// free-text message, under the same precedence `value_lines` uses.
fn has_value_decision(path: &[&str], arg: &Arg) -> bool {
    if !arg.get_possible_values().is_empty() {
        return true;
    }
    let id = arg.get_id().to_string();
    if let Some(kind) = kinds::lookup_exact(path, &id) {
        return !matches!(kind, Kind::FreeText);
    }
    if hint_line(arg).is_some() {
        return true;
    }
    matches!(
        kinds::lookup_generic(&id),
        Some(kind) if !matches!(kind, Kind::FreeText)
    )
}

/// `!message <VALUE_NAME> — <arg help>` for free-text slots.
fn free_text_message(arg: &Arg) -> String {
    let name = arg
        .get_value_names()
        .and_then(|names| names.first())
        .map(|name| name.to_string())
        .unwrap_or_else(|| arg.get_id().to_string().to_uppercase());
    match arg.get_help() {
        Some(help) if !help.to_string().is_empty() => {
            format!("{name} \u{2014} {}", help)
        }
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use std::ffi::OsString;

    fn request(words: &[&str]) -> Request {
        protocol::parse_request(
            &[
                vec![
                    OsString::from("zsh"),
                    OsString::from("--protocol"),
                    OsString::from("1"),
                    OsString::from("--"),
                ],
                words.iter().map(OsString::from).collect::<Vec<_>>(),
            ]
            .concat(),
        )
        .expect("test request parses")
    }

    fn lines(words: &[&str]) -> Vec<String> {
        complete_request(&request(words))
    }

    fn values_of(lines: &[String]) -> Vec<&str> {
        lines
            .iter()
            .filter(|line| !line.starts_with('!'))
            .map(|line| line.split('\t').next().unwrap_or_default())
            .collect()
    }

    fn groups_of(lines: &[String]) -> Vec<&str> {
        lines
            .iter()
            .filter(|line| !line.starts_with('!'))
            .map(|line| line.split('\t').nth(2).unwrap_or_default())
            .collect()
    }

    #[test]
    fn root_empty_offers_commands_then_capture_protocol() {
        let output = lines(&["bob", ""]);
        assert!(!output.is_empty());
        let values = values_of(&output);
        assert!(values.contains(&"capture"));
        assert!(values.contains(&"freshness"));
        assert!(values.contains(&"vault-sync"));
        assert!(values.contains(&"capture-complete"));
        assert!(!values.iter().any(|value| value.starts_with('-')));
        let first_plumbing = values
            .iter()
            .position(|value| *value == "capture-complete")
            .expect("plumbing present");
        let last_porcelain = values
            .iter()
            .position(|value| *value == "vault-sync")
            .expect("porcelain present");
        assert!(last_porcelain < first_plumbing);
        assert!(groups_of(&output).contains(&"capture protocol"));
        assert!(!values.contains(&"mark-next-tasks"));
    }

    #[test]
    fn lone_dash_pairs_forms_with_identical_descriptions() {
        let output = lines(&["bob", "capture", "-"]);
        let rows: Vec<Vec<&str>> = output
            .iter()
            .map(|line| line.split('\t').collect())
            .collect();
        let short = rows
            .iter()
            .position(|row| row[0] == "-b")
            .expect("short present");
        assert_eq!(rows[short + 1][0], "--bob-dir");
        assert_eq!(rows[short][1], rows[short + 1][1]);
        assert!(!rows[short][1].is_empty());
    }

    #[test]
    fn double_dash_offers_long_forms_only() {
        let output = lines(&["bob", "capture", "--"]);
        let values = values_of(&output);
        assert!(values.contains(&"--bob-dir"));
        assert!(!values.iter().any(|value| {
            value.starts_with('-') && !value.starts_with("--")
        }));
    }

    #[test]
    fn attached_option_values_carry_prefix() {
        let output = lines(&["bob", "capture", "--format="]);
        assert_eq!(output[0], "!prefix 9");
        let values = values_of(&output);
        assert!(values.contains(&"human"));
        assert!(values.contains(&"json"));
    }

    #[test]
    fn text_started_slot_offers_no_options() {
        // Since the capture-text phase, the capture trio's TEXT goes through
        // the live marker extraction: a dash word with no marker offers
        // nothing (no options per rule 7, no interim message).
        let output = lines(&["bob", "capture", "fix", "-"]);
        assert!(output.is_empty(), "{output:?}");
    }

    #[test]
    fn structural_latency_is_warn_only() {
        // Median over in-process structural requests; warns, never fails.
        let cases: &[&[&str]] = &[
            &["bob", ""],
            &["bob", "capture", ""],
            &["bob", "capture", "--format", ""],
            &["bob", "gkeep", "pull", "--format", ""],
        ];
        let mut samples = Vec::new();
        for _ in 0..31 {
            for case in cases {
                let started = Instant::now();
                let _ = lines(case);
                samples.push(started.elapsed());
            }
        }
        samples.sort();
        let p50 = samples[samples.len() / 2];
        if p50.as_millis() > 10 {
            eprintln!(
                "warning: completion p50 {p50:?} exceeds 10 ms over {} samples",
                samples.len()
            );
        }
    }
}
