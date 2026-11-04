//! Shared lexer for a first-level move bullet in a sealed project-note item.
//!
//! A first-level bullet whose body is only `^<id>` or `:<id>` moves that open
//! task from the parent note into the new note. Both the execution parser
//! (`item.rs`) and the editor parser (`editor_parse.rs`) claim the bullet
//! from its raw line shape before `strip_trailing_task_id` runs, so `bob
//! capture` and `capture-parse` agree on every rule; a claimed line never
//! reaches the new-task path. Escapes, inline code, and wikilinks keep
//! precedence: `- \^a` stays a literal `^a` task. Error constructors live
//! here so both paths report byte-identical messages.

use super::project_tasks::is_reserved_project_task_id;
use super::tokens::is_block_id;
use std::ops::Range;

/// One move-bullet classification on a first-level authored body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveBulletHit {
    /// A well-formed `^<id>` / `:<id>` token with nothing after it.
    Complete {
        sigil: char,
        id: String,
        sigil_span: (usize, usize),
        id_span: (usize, usize),
        span: Range<usize>,
    },
    /// A lone `^` / `:` with nothing after it: the editor opens the move
    /// picker, and `bob capture` refuses it.
    Unfinished { sigil: char, span: Range<usize> },
    /// An exact `- ^^` choice marker: answered after Return, before any
    /// write. The span covers both carets.
    Choice { span: Range<usize> },
    /// A move-shaped line that breaks a lexical rule. The span covers the
    /// sigil plus its first word.
    Invalid {
        code: &'static str,
        kind: MoveBulletInvalid,
        span: Range<usize>,
    },
}

/// Which lexical rule a move-shaped line breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveBulletInvalid {
    /// Words after the move token.
    Text { sigil: char, id: String },
    /// A schedule, priority, directive, type, or route marker after it.
    Marker { token: String },
    /// A leading checkbox before it.
    Checkbox { status: char },
    /// A `^route:id` / `:route:id` cross-note shape.
    CrossNote { sigil: char, id: String },
    /// A reserved `^prj` / `:prj` in any letter case.
    Reserved { sigil: char, id: String },
    /// A first word that starts like an ID but fails the block-ID charset.
    BadId { raw: String },
    /// Words after a `- ^^` choice marker.
    ChoiceText,
    /// A `- ::` line, which teaches `- ^^`.
    ChoiceColon,
}

/// Scan a first-level authored body (bullet marker already stripped) for a
/// move bullet. `base` is the absolute start of `body`, and `source` is the
/// whole draft for protected-markup checks. Returns `None` when the line is
/// not move-shaped, so the normal path handles it unchanged.
pub(crate) fn scan_move_bullet(
    body: &str,
    base: usize,
    source: &str,
) -> Option<MoveBulletHit> {
    let trimmed = body.trim_start();
    let mut cursor = body.len() - trimmed.len();
    if trimmed.starts_with('\\') {
        return None;
    }
    let (checkbox, after_checkbox) =
        super::project_tasks::split_leading_checkbox(trimmed);
    cursor += trimmed.len() - after_checkbox.len();
    let rest = after_checkbox;
    let sigil = rest.as_bytes().first()?;
    if !matches!(sigil, b'^' | b':') {
        return None;
    }
    let sigil = *sigil as char;
    let sigil_abs = base + cursor;
    if crate::native::capture_links::is_protected_markup_range(
        source,
        sigil_abs,
        sigil_abs + 1,
    ) {
        return None;
    }
    let after = &rest[1..];
    // A doubled sigil claims the line before the alphanumeric check below,
    // so `- ^^` never falls through as prose. `- ::` is the reserved shape
    // pointing at `- ^^`.
    if after.as_bytes().first() == Some(&(sigil as u8)) {
        let first_word_end = token_end(rest, 0);
        let first_word = &rest[..first_word_end];
        let span = sigil_abs..sigil_abs + first_word_end;
        let remainder = &rest[first_word_end..];
        // A checkbox still reports first, like every other move shape,
        // but only for exact choice shapes; `::x` stays prose.
        if let Some(status) = checkbox {
            let exact = if sigil == '^' {
                first_word.starts_with("^^")
            } else {
                first_word == "::" && remainder.trim().is_empty()
            };
            if exact {
                return Some(MoveBulletHit::Invalid {
                    code: "move_bullet_checkbox",
                    kind: MoveBulletInvalid::Checkbox { status },
                    span,
                });
            }
            return None;
        }
        if sigil == ':' {
            // Only an exact `- ::` teaches `- ^^`; `::x` and friends keep
            // today's prose meaning.
            if first_word == "::" && remainder.trim().is_empty() {
                return Some(MoveBulletHit::Invalid {
                    code: "move_choice_colon",
                    kind: MoveBulletInvalid::ChoiceColon,
                    span,
                });
            }
            return None;
        }
        if first_word == "^^" && remainder.trim().is_empty() {
            let span = sigil_abs..sigil_abs + 2;
            return Some(MoveBulletHit::Choice { span });
        }
        // Any `^^`-prefixed first word (`^^^`, `^^x`, `^^ text`) teaches
        // the exact shape.
        if first_word.starts_with("^^") {
            return Some(MoveBulletHit::Invalid {
                code: "move_choice_text",
                kind: MoveBulletInvalid::ChoiceText,
                span,
            });
        }
        return None;
    }
    let after_trimmed = after.trim_start();
    if after_trimmed.is_empty() {
        let span = sigil_abs..sigil_abs + 1;
        if let Some(status) = checkbox {
            return Some(MoveBulletHit::Invalid {
                code: "move_bullet_checkbox",
                kind: MoveBulletInvalid::Checkbox { status },
                span,
            });
        }
        return Some(MoveBulletHit::Unfinished { sigil, span });
    }
    if after_trimmed.len() != after.len() {
        // A lone sigil followed by other words stays prose.
        return None;
    }
    let word_end = token_end(after, 0);
    let word = &after[..word_end];
    if !word.as_bytes()[0].is_ascii_alphanumeric() {
        return None;
    }
    if let Some(cross) = cross_note_id(word) {
        let span = sigil_abs..sigil_abs + 1 + word_end;
        if let Some(status) = checkbox {
            // The checkbox comes first in the line, so it is the first
            // problem even on a cross-note shape.
            return Some(MoveBulletHit::Invalid {
                code: "move_bullet_checkbox",
                kind: MoveBulletInvalid::Checkbox { status },
                span,
            });
        }
        return Some(MoveBulletHit::Invalid {
            code: "move_bullet_cross_note",
            kind: MoveBulletInvalid::CrossNote {
                sigil,
                id: cross.to_string(),
            },
            span,
        });
    }
    if !is_block_id(word) {
        let span = sigil_abs..sigil_abs + 1 + word_end;
        if let Some(status) = checkbox {
            return Some(MoveBulletHit::Invalid {
                code: "move_bullet_checkbox",
                kind: MoveBulletInvalid::Checkbox { status },
                span,
            });
        }
        return Some(MoveBulletHit::Invalid {
            code: "invalid_project_task_id",
            kind: MoveBulletInvalid::BadId {
                raw: word.to_string(),
            },
            span,
        });
    }
    if is_reserved_project_task_id(word) {
        let span = sigil_abs..sigil_abs + 1 + word_end;
        if let Some(status) = checkbox {
            return Some(MoveBulletHit::Invalid {
                code: "move_bullet_checkbox",
                kind: MoveBulletInvalid::Checkbox { status },
                span,
            });
        }
        return Some(MoveBulletHit::Invalid {
            code: "move_bullet_reserved",
            kind: MoveBulletInvalid::Reserved {
                sigil,
                id: word.to_string(),
            },
            span,
        });
    }
    let span = sigil_abs..sigil_abs + 1 + word_end;
    if let Some(status) = checkbox {
        return Some(MoveBulletHit::Invalid {
            code: "move_bullet_checkbox",
            kind: MoveBulletInvalid::Checkbox { status },
            span,
        });
    }
    let rest = after[word_end..].trim_start();
    if rest.is_empty() {
        let id_start = sigil_abs + 1;
        return Some(MoveBulletHit::Complete {
            sigil,
            id: word.to_string(),
            sigil_span: (sigil_abs, sigil_abs + 1),
            id_span: (id_start, id_start + word.len()),
            span,
        });
    }
    let rest_word_end = token_end(rest, 0);
    let rest_word = &rest[..rest_word_end];
    if is_marker_like(rest_word) {
        return Some(MoveBulletHit::Invalid {
            code: "move_bullet_marker",
            kind: MoveBulletInvalid::Marker {
                token: rest_word.to_string(),
            },
            span,
        });
    }
    Some(MoveBulletHit::Invalid {
        code: "move_bullet_text",
        kind: MoveBulletInvalid::Text {
            sigil,
            id: word.to_string(),
        },
        span,
    })
}

/// Split a `route:id` word into its cross-note ID when the head looks like a
/// route. Returns `None` for ordinary words.
fn cross_note_id(word: &str) -> Option<&str> {
    let colon = word.find(':')?;
    let head = &word[..colon];
    if head.is_empty()
        || !head.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
    {
        return None;
    }
    Some(&word[colon + 1..])
}

/// Whether the first word after a move token is itself a capture marker.
fn is_marker_like(word: &str) -> bool {
    super::markers::parse_schedule_token(word).is_some()
        || super::markers::parse_priority_token(word).is_some()
        || super::markers::parse_retired_priority_token(word).is_some()
        || word.starts_with(['%', '#', '@'])
}

fn token_end(line: &str, start: usize) -> usize {
    let mut end = start;
    while end < line.len()
        && !line[end..].chars().next().unwrap().is_whitespace()
    {
        end += line[end..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    end
}

pub(crate) fn move_bullet_text_error(
    sigil: char,
    id: &str,
    route: &str,
) -> String {
    format!(
        "`- {sigil}{id}` moves a task from {route}.md and takes no other \
text; to write a new task, put the ID last: `- tweak it {sigil}{id}`"
    )
}

pub(crate) fn move_bullet_marker_error(token: &str) -> String {
    format!(
        "a moved task keeps its own schedule, priority, and type; remove \
{token}"
    )
}

pub(crate) fn move_bullet_checkbox_error(status: char) -> String {
    format!("a moved task keeps its own checkbox; remove [{status}]")
}

/// Notes under a moved task are staged after capture under the new note's
/// task-plus selector. `new_note` is the new note's stem (for example
/// `cash_goog_exit`).
pub(crate) fn move_bullet_nested_error(
    new_note: &str,
    move_id: &str,
) -> String {
    format!(
        "notes under a moved task are not supported yet; add them after \
capture with @{new_note}+{move_id}"
    )
}

pub(crate) fn move_bullet_duplicate_error(
    sigil: char,
    id: &str,
    first_line: usize,
) -> String {
    format!("`{sigil}{id}` is already moved on line {first_line}")
}

pub(crate) fn move_bullet_id_collision_error(
    id: &str,
    route: &str,
    move_line: usize,
    new_task_line: usize,
) -> String {
    format!(
        "task ID `{id}` is moved from {route}.md on line {move_line}; rename \
the new task on line {new_task_line}"
    )
}

pub(crate) fn move_bullet_cross_note_error(
    sigil: char,
    id: &str,
    route: &str,
) -> String {
    format!(
        "moving from another note is not supported yet; `- {sigil}{id}` \
moves from {route}.md"
    )
}

pub(crate) fn move_bullet_reserved_error(
    sigil: char,
    id: &str,
    route: &str,
) -> String {
    format!(
        "`{sigil}{id}` is a project's own task and never moves; pick one of \
{route}.md's open tasks"
    )
}

pub(crate) fn move_bullet_unfinished_error(sigil: char, route: &str) -> String {
    format!(
        "`- {sigil}` needs a task from {route}.md; pick one or delete the \
bullet"
    )
}

pub(crate) fn move_choice_text_error(route: &str) -> String {
    format!(
        "`- ^^` takes no other text; leave it alone on its line to choose \
tasks from {route}.md"
    )
}

pub(crate) fn move_choice_nested_error() -> String {
    "notes under `- ^^` are not supported; add them after capture".to_string()
}

pub(crate) fn move_choice_duplicate_error(first_line: usize) -> String {
    format!("only one `- ^^` per capture; the first is on line {first_line}")
}

pub(crate) fn move_choice_colon_error() -> String {
    "`- ::` is not a picker; write `- ^^` and choose `^` or `:` for each task"
        .to_string()
}

/// Map a move-bullet refusal message back to its diagnostic code, mirroring
/// how the execution parser strings its errors while the editor parser sets
/// codes directly.
pub(crate) fn code_for_message(message: &str) -> Option<&'static str> {
    if (message.starts_with("`- ^") || message.starts_with("`- :"))
        && message.contains("moves a task from ")
        && message.contains("takes no other text")
    {
        Some("move_bullet_text")
    } else if message.starts_with(
        "a moved task keeps its own schedule, priority, and type; remove ",
    ) {
        Some("move_bullet_marker")
    } else if message.starts_with("a moved task keeps its own checkbox; remove [")
        && message.ends_with(']')
    {
        Some("move_bullet_checkbox")
    } else if message.starts_with(
        "notes under a moved task are not supported yet; add them after capture with @",
    ) {
        Some("move_bullet_nested")
    } else if message.contains("` is already moved on line ") {
        Some("move_bullet_duplicate")
    } else if message.starts_with("task ID `")
        && message.contains("is moved from ")
        && message.contains("; rename the new task on line ")
    {
        Some("move_bullet_id_collision")
    } else if message
        .starts_with("moving from another note is not supported yet; ")
    {
        Some("move_bullet_cross_note")
    } else if message.contains("is a project's own task and never moves; pick one of ")
        && message.ends_with("open tasks")
    {
        Some("move_bullet_reserved")
    } else if message.contains("needs a task from ")
        && message.contains("; pick one or delete the bullet")
    {
        Some("move_bullet_unfinished")
    } else if message.starts_with("`- ^^` takes no other text; leave it alone")
        && message.contains("to choose tasks from ")
    {
        Some("move_choice_text")
    } else if message
        == "notes under `- ^^` are not supported; add them after capture"
    {
        Some("move_choice_nested")
    } else if message.starts_with("only one `- ^^` per capture; the first is on line ") {
        Some("move_choice_duplicate")
    } else if message
        == "`- ::` is not a picker; write `- ^^` and choose `^` or `:` for each task"
    {
        Some("move_choice_colon")
    } else {
        None
    }
}
