//! Shared terminal `z:<when>` snooze intent and the narrow `@route:id!`
//! snooze-drop spelling.
//!
//! Lex these shapes and carry raw intent to the capture transaction adapter.
//! Clock-dependent validation (past, next-day, DST gap, malformed times)
//! belongs to the shared time-resolution boundary the mutation phase
//! calls, never to a second lexer here: a claimed `z:<non-empty>` token
//! is lexically complete however its value reads.

use super::markers::is_route_token;
use super::tokens::is_block_id;

/// Lowercase terminal snooze prefix. Uppercase `Z:` never claims: it stays
/// literal task text, matching the at-most-one-lowercase rule.
pub(crate) const SNOOZE_PREFIX: &str = "z:";

/// Diagnostic code for a bare `z:` with no value.
pub(crate) const INCOMPLETE_SNOOZE_CODE: &str = "incomplete_snooze";

/// Diagnostic code for every snooze shape the grammar refuses. One code
/// covers all incompatibilities; each message teaches its own fix.
pub(crate) const INVALID_SNOOZE_CODE: &str = "invalid_snooze";

/// Label used by the duplicate-marker machinery for a second `z:`.
pub(crate) const SNOOZE_MARKER_LABEL: &str = "snooze marker (z:<when>)";

/// A lexed snooze intent: an add (`z:<when>`) or a narrow drop
/// (`@route:id!`). `raw` is the text after `z:`, empty for a bare `z:`
/// and for a drop. `span` covers the full claimed marker in UTF-8 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnoozeIntent {
    pub(crate) raw: String,
    pub(crate) span: Option<(usize, usize)>,
    pub(crate) drop: bool,
}

impl SnoozeIntent {
    pub(crate) fn add(raw: String, span: Option<(usize, usize)>) -> Self {
        Self {
            raw,
            span,
            drop: false,
        }
    }

    pub(crate) fn drop(span: Option<(usize, usize)>) -> Self {
        Self {
            raw: String::new(),
            span,
            drop: true,
        }
    }

    /// A bare `z:` claims the marker but names no time yet.
    pub(crate) fn is_bare(&self) -> bool {
        !self.drop && self.raw.is_empty()
    }
}

/// True when the token claims the terminal snooze slot: exactly the
/// lowercase `z:` prefix, with any (possibly empty) value. Uppercase
/// `Z:`, code-shaped `` `z:19` `` (backticks break the prefix), and
/// mid-line prose never reach this predicate as terminal candidates;
/// extraction only inspects terminal-region tokens.
pub(crate) fn is_snooze_token(text: &str) -> bool {
    text == SNOOZE_PREFIX || text.starts_with(SNOOZE_PREFIX)
}

/// Split a claimed snooze token into its raw value: `Some` for
/// `z:<non-empty>`, `None` for bare `z:`. Callers check
/// [`is_snooze_token`] first.
pub(crate) fn snooze_value(token: &str) -> Option<&str> {
    let value = token.strip_prefix(SNOOZE_PREFIX)?;
    (!value.is_empty()).then_some(value)
}

/// A narrow snooze-drop target: `@<route>:<block-id>` plus the terminal
/// `!`. Distinct from the existing `@route+id!` toggle: the colon names
/// the snoozed row's task, and only the row is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnoozeDropTarget {
    pub(crate) route: String,
    pub(crate) block_id: String,
}

/// Parse the narrow `@<route>:<block-id>!` drop spelling, or `None` for
/// every other token. Runs before ordinary target validation so the
/// trailing `!` never reaches block-ID validation, which stays as strict
/// as before: route and block ID use the established validators, and
/// any `#`, `=`, `+`, second `!`, or `@` byte fails the shape back to
/// the existing errors. In particular `@route+id!` (toggle),
/// `@r:id=x!` (close), `@r:id#name!`, and `!note:id!` never claim.
pub(crate) fn parse_snooze_drop_token(token: &str) -> Option<SnoozeDropTarget> {
    let body = token.strip_suffix('!')?;
    if body.contains('!') || body.starts_with("@@") {
        return None;
    }
    let marker = body.strip_prefix('@')?;
    if marker.starts_with('@') || marker.starts_with('!') {
        return None;
    }
    let (route, block_id) = marker.split_once(':')?;
    if route.is_empty()
        || block_id.is_empty()
        || !is_route_token(route)
        || !is_block_id(block_id)
    {
        return None;
    }
    if block_id.contains(['#', '=', '+', '@', ':', '!', ' ']) {
        return None;
    }
    if route.contains(['#', '=', '+', '@', ':', '!', ' ']) {
        return None;
    }
    Some(SnoozeDropTarget {
        route: route.to_ascii_lowercase(),
        block_id: block_id.to_string(),
    })
}

/// Bare `z:` names no time yet.
pub(crate) fn incomplete_snooze_error(token: &str) -> String {
    format!(
        "`{token}` is incomplete: add the snooze time (`z:1530`, `z:19`, or `z:+1h`)"
    )
}

/// Human name for a conflicting marker in snooze teaching copy.
pub(crate) fn snooze_schedule_conflict_error() -> String {
    "snooze `z:<when>` cannot be combined with `s:<N>`; drop one of them (`z:` snoozes until today at that time, `s:` defers to another day)".to_string()
}

pub(crate) fn snooze_priority_conflict_error() -> String {
    "snooze `z:<when>` cannot be combined with `$<N>`; drop one of them"
        .to_string()
}

pub(crate) fn snooze_start_conflict_error() -> String {
    "snooze `z:<when>` cannot be combined with `=<X>`; drop one of them"
        .to_string()
}

pub(crate) fn snooze_close_conflict_error() -> String {
    "snooze `z:<when>` cannot be combined with `=x`; drop one of them"
        .to_string()
}

pub(crate) fn snooze_drop_conflict_error() -> String {
    "snooze `z:<when>` cannot be combined with `!`; drop the snooze row with `@route:id!` alone or snooze with `z:<when>` alone"
        .to_string()
}

pub(crate) fn snooze_session_conflict_error() -> String {
    "snooze `z:<when>` cannot be combined with `#<pomodoro>`; drop one of them"
        .to_string()
}

/// A `z:` on an item kind that has no snoozable task target.
pub(crate) fn snooze_unsupported_kind_error(detail: &str) -> String {
    format!("snooze `z:<when>` needs a task to snooze; {detail}")
}

/// An existing-task snooze carries no authored children: they would have
/// nowhere to land until the task is linked.
pub(crate) fn snooze_existing_children_error() -> String {
    "an existing-task snooze takes no authored children; link the task first (`@route:id`), then add children to it".to_string()
}

/// An existing-task snooze carries no clipboard: there is no new task to
/// fill.
pub(crate) fn snooze_existing_clip_error() -> String {
    "an existing-task snooze takes no `%c`; link the task first (`@route:id`), then capture the clipboard as its own item".to_string()
}

/// Snooze takes one task: a multi-target run cannot share one `z:`.
pub(crate) fn snooze_multi_target_error() -> String {
    "snooze `z:<when>` takes one task; put each `@route+id` target on its own item"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_token_claims_only_lowercase() {
        assert!(is_snooze_token("z:"));
        assert!(is_snooze_token("z:1530"));
        assert!(is_snooze_token("z:+1h"));
        assert!(!is_snooze_token("Z:1530"));
        assert!(!is_snooze_token("s:2"));
        assert!(!is_snooze_token("z"));
        assert_eq!(snooze_value("z:1530"), Some("1530"));
        assert_eq!(snooze_value("z:+1h30m"), Some("+1h30m"));
        assert_eq!(snooze_value("z:"), None);
    }

    #[test]
    fn drop_claims_only_the_narrow_spelling() {
        let drop = parse_snooze_drop_token("@job:email-dan-eod!")
            .expect("narrow drop claims");
        assert_eq!(drop.route, "job");
        assert_eq!(drop.block_id, "email-dan-eod");
        // Toggle, drawer, and malformed shapes stay unclaimed. (`=x!` is
        // a valid close with a complete-all group, not a drop near miss.)
        for token in [
            "@job:email-dan-eod+!",
            "@job:email-dan-eod!x",
            "@job:email-dan-eod#sec!",
            "@job:email-dan-eod!!",
            "@@job:email-dan-eod!",
            "@!job:email-dan-eod!",
            "@job:email-dan-eod",
            "@job:email-dan-eod! #sec",
            "@job::x!",
            "@:x!",
            "@job:! ",
            "job:email!",
        ] {
            assert!(
                parse_snooze_drop_token(token).is_none(),
                "{token} must not claim the drop"
            );
        }
    }
}
