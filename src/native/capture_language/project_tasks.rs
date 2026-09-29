//! Project-note task-ID lexer and shared rule checks.
//!
//! A trailing ` :id` / ` ^id` token on a project-note line names that task
//! bullet (`:` additionally makes it Next and links it into the Pomodoro).
//! Both the execution parser (`item.rs`) and the editor parser
//! (`editor_parse.rs`) run the same post-pass through this module once the
//! item's kind is known to be `ProjectNote`, so `bob capture` and
//! `capture-parse` agree on every rule. Error constructors live here so
//! both paths report byte-identical messages.

use super::model::*;

/// One lexed trailing task-ID candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskIdLex {
    /// A lone `:` or `^`: an unfinished ID the editor reports as
    /// `incomplete` needing `block_id`.
    Lone { sigil: char },
    /// A well-shaped ` :id` / ` ^id` token.
    Valid { sigil: char, id: String },
    /// Starts with an ASCII letter or digit after the sigil but fails
    /// `is_block_id` (for example `:a_b`). `raw` is the ID text after the
    /// sigil, exactly as typed.
    Invalid { sigil: char, raw: String },
}

/// Lex one whitespace-free trailing token as a project task ID candidate.
///
/// A task ID token is a sigil (`:` or `^`) followed by at least one
/// character, where the first character after the sigil is an ASCII letter
/// or digit -- so `:)`, `:-)`, `:(`, `::x`, and `10:30` stay prose.
/// Returns `None` for prose.
pub(crate) fn lex_project_task_id(token: &str) -> Option<TaskIdLex> {
    if token == ":" {
        return Some(TaskIdLex::Lone { sigil: ':' });
    }
    if token == "^" {
        return Some(TaskIdLex::Lone { sigil: '^' });
    }
    let (sigil, rest) = match token.strip_prefix(':') {
        Some(rest) => (':', rest),
        None => ('^', token.strip_prefix('^')?),
    };
    let first = rest.as_bytes().first()?;
    if !first.is_ascii_alphanumeric() {
        return None;
    }
    if super::tokens::is_block_id(rest) {
        Some(TaskIdLex::Valid {
            sigil,
            id: rest.to_string(),
        })
    } else {
        Some(TaskIdLex::Invalid {
            sigil,
            raw: rest.to_string(),
        })
    }
}

/// Whether a lexed task ID is the reserved `prj` name in any letter case.
pub(crate) fn is_reserved_project_task_id(id: &str) -> bool {
    id.eq_ignore_ascii_case("prj")
}

/// The last whitespace-separated word of a marker-stripped line body, or
/// `None` when the body holds no words.
pub(crate) fn last_body_word(body: &str) -> Option<&str> {
    body.split_whitespace().next_back()
}

/// Remove the trailing task-ID word from a marker-stripped line body. The
/// caller must have lexed the last word as a task ID already.
pub(crate) fn strip_task_id_suffix(body: &str) -> String {
    match body.rsplit_once(char::is_whitespace) {
        Some((head, _)) => head.to_string(),
        None => String::new(),
    }
}

/// Split a leading `[X]` checkbox marker off a trimmed authored body,
/// returning the status character and the remaining body. Returns
/// `(None, body)` when no checkbox is present.
///
/// This is the single shared helper for the grammar's `:`-checkbox rule and
/// the project-note renderer, so both agree on the shape.
pub(crate) fn split_leading_checkbox(trimmed: &str) -> (Option<char>, &str) {
    let mut chars = trimmed.char_indices();
    if chars.next().map(|(_, value)| value) != Some('[') {
        return (None, trimmed);
    }
    let Some((_, status)) = chars.next() else {
        return (None, trimmed);
    };
    if status == ']' || status == '\n' {
        return (None, trimmed);
    }
    let Some((close_start, close)) = chars.next() else {
        return (None, trimmed);
    };
    if close != ']' {
        return (None, trimmed);
    }
    let rest = &trimmed[close_start + 1..];
    if rest.is_empty() {
        return (Some(status), "");
    }
    if !rest.starts_with(char::is_whitespace) {
        return (None, trimmed);
    }
    (Some(status), rest.trim_start())
}

pub(crate) fn invalid_project_task_id_charset_error(raw: &str) -> String {
    format!("task ID `{raw}` may use only A-Z, a-z, 0-9 or '-'")
}

pub(crate) fn reserved_project_task_id_error(raw: &str) -> String {
    format!(
        "task ID `{raw}` is reserved for the project's own `^prj` task; choose another ID"
    )
}

/// A well-formed or invalid task ID token on the project note's own parent
/// line. `token` is the typed token including its sigil.
pub(crate) fn misplaced_parent_task_id_error(token: &str) -> String {
    format!(
        "the project's own task is always `^prj` and is never linked; put `{token}` on a task bullet"
    )
}

/// A well-formed or invalid task ID token on a nested bullet. `token` is
/// the typed token including its sigil.
pub(crate) fn misplaced_nested_task_id_error(token: &str) -> String {
    format!(
        "task IDs go on first-level task bullets; move `{token}` to a first-level bullet"
    )
}

pub(crate) fn empty_project_task_body_error(line_number: usize) -> String {
    format!("capture line {line_number} names a task but has no task text")
}

pub(crate) fn checkbox_project_task_id_error(id: &str, status: char) -> String {
    format!(
        "` :{id}` makes the task Next and links it, so it takes no `[{status}]` checkbox; remove the checkbox or write ` ^{id}` to keep it"
    )
}

pub(crate) fn duplicate_project_task_id_error(
    id: &str,
    first_line: usize,
    second_line: usize,
) -> String {
    format!(
        "task ID `{id}` is already used on line {first_line}; rename the task on line {second_line}"
    )
}

pub(crate) fn unfinished_project_task_id_error(sigil: char) -> String {
    format!(
        "` {sigil}` needs a task ID: end the bullet with ` {sigil}<block-id>`"
    )
}

/// One first-level-or-nested child's task-ID decision, in the plan's
/// evaluation order: shape and charset, then reserved, then placement, then
/// empty body, then checkbox, then duplicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChildTaskOutcome {
    /// Prose, or a lone sigil on a nested bullet: leave the body alone.
    Ignore,
    /// A lone sigil ending a first-level bullet.
    Unfinished { sigil: char },
    /// An accepted ID: `stripped` is the body without the token.
    Accept {
        sigil: char,
        id: String,
        stripped: String,
    },
    /// A rule violation with its diagnostic code and execution message.
    Error { code: &'static str, message: String },
}

/// Stateful per-item task-ID pass shared by the execution and editor
/// parsers. Feed the parent body, then each child in source order; both
/// paths implement the same evaluation order through this one type so
/// `bob capture` and `capture-parse` can never disagree.
#[derive(Debug, Default)]
pub(crate) struct ProjectTaskPass {
    seen: Vec<(String, usize)>,
    pub(crate) has_link: bool,
}

impl ProjectTaskPass {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Step 2 of the evaluation order: the parent line. A well-formed or
    /// invalid task ID token is misplaced; a lone sigil stays literal.
    pub(crate) fn check_parent(&self, body: &str) -> Option<String> {
        let token = last_body_word(body)?;
        match lex_project_task_id(token)? {
            TaskIdLex::Valid { .. } | TaskIdLex::Invalid { .. } => {
                Some(misplaced_parent_task_id_error(token))
            }
            TaskIdLex::Lone { .. } => None,
        }
    }

    /// Step 3: one child in source order.
    pub(crate) fn check_child(
        &mut self,
        body: &str,
        depth: AuthoredDepth,
        line_number: usize,
    ) -> ChildTaskOutcome {
        let Some(token) = last_body_word(body) else {
            return ChildTaskOutcome::Ignore;
        };
        match lex_project_task_id(token) {
            None => ChildTaskOutcome::Ignore,
            Some(TaskIdLex::Lone { sigil }) => {
                if depth == AuthoredDepth::First {
                    ChildTaskOutcome::Unfinished { sigil }
                } else {
                    ChildTaskOutcome::Ignore
                }
            }
            Some(TaskIdLex::Invalid { raw, .. }) => ChildTaskOutcome::Error {
                code: "invalid_project_task_id",
                message: invalid_project_task_id_charset_error(&raw),
            },
            Some(TaskIdLex::Valid { sigil, id }) => {
                if is_reserved_project_task_id(&id) {
                    return ChildTaskOutcome::Error {
                        code: "invalid_project_task_id",
                        message: reserved_project_task_id_error(&id),
                    };
                }
                if depth != AuthoredDepth::First {
                    return ChildTaskOutcome::Error {
                        code: "misplaced_project_task_id",
                        message: misplaced_nested_task_id_error(token),
                    };
                }
                let stripped = strip_task_id_suffix(body);
                if stripped.split_whitespace().next().is_none() {
                    return ChildTaskOutcome::Error {
                        code: "invalid_project_task_id",
                        message: empty_project_task_body_error(line_number),
                    };
                }
                let link = sigil == ':';
                if link {
                    let (checkbox, _) =
                        split_leading_checkbox(stripped.trim_start());
                    if let Some(status) = checkbox {
                        return ChildTaskOutcome::Error {
                            code: "invalid_project_task_id",
                            message: checkbox_project_task_id_error(
                                &id, status,
                            ),
                        };
                    }
                }
                if let Some((_, first)) =
                    self.seen.iter().find(|(seen_id, _)| seen_id == &id)
                {
                    return ChildTaskOutcome::Error {
                        code: "duplicate_project_task_id",
                        message: duplicate_project_task_id_error(
                            &id,
                            *first,
                            line_number,
                        ),
                    };
                }
                self.seen.push((id.clone(), line_number));
                if link {
                    self.has_link = true;
                }
                ChildTaskOutcome::Accept {
                    sigil,
                    id,
                    stripped,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_accepts_the_boundary_set() {
        // (token, expected lex)
        let cases: &[(&str, Option<TaskIdLex>)] = &[
            (
                ":D",
                Some(TaskIdLex::Valid {
                    sigil: ':',
                    id: "D".to_string(),
                }),
            ),
            (":)", None),
            (":-)", None),
            (":(", None),
            (
                ":draft-memo",
                Some(TaskIdLex::Valid {
                    sigil: ':',
                    id: "draft-memo".to_string(),
                }),
            ),
            (
                "^Equity-2",
                Some(TaskIdLex::Valid {
                    sigil: '^',
                    id: "Equity-2".to_string(),
                }),
            ),
            (
                ":a_b",
                Some(TaskIdLex::Invalid {
                    sigil: ':',
                    raw: "a_b".to_string(),
                }),
            ),
            (":", Some(TaskIdLex::Lone { sigil: ':' })),
            ("^", Some(TaskIdLex::Lone { sigil: '^' })),
            ("::x", None),
            (
                ":x:",
                Some(TaskIdLex::Invalid {
                    sigil: ':',
                    raw: "x:".to_string(),
                }),
            ),
            ("10:30", None),
            ("draft-memo", None),
            ("", None),
            (
                ":1",
                Some(TaskIdLex::Valid {
                    sigil: ':',
                    id: "1".to_string(),
                }),
            ),
        ];
        for (token, expected) in cases {
            assert_eq!(lex_project_task_id(token), *expected, "{token}");
        }
    }

    #[test]
    fn reserved_name_matches_in_any_letter_case() {
        for id in ["prj", "PRJ", "Prj", "pRj"] {
            assert!(is_reserved_project_task_id(id), "{id}");
        }
        for id in ["pr", "prjx", "draft-memo", "1"] {
            assert!(!is_reserved_project_task_id(id), "{id}");
        }
    }

    #[test]
    fn suffix_helpers_split_on_the_last_word() {
        assert_eq!(
            last_body_word("Draft the memo :draft-memo"),
            Some(":draft-memo")
        );
        assert_eq!(last_body_word("  spaced   body  "), Some("body"));
        assert_eq!(last_body_word(""), None);
        assert_eq!(
            strip_task_id_suffix("Draft the memo :draft-memo"),
            "Draft the memo"
        );
        assert_eq!(strip_task_id_suffix(":foo"), "");
    }
}
