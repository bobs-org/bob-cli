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
#[cfg(test)]
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

/// One trailing task-ID token stripped from a token stream before marker
/// extraction, with its original token for spans. The ID token is removed
/// so schedule/priority markers behind it still extract; validation runs on
/// the returned lex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StrippedTaskId<'a> {
    pub(crate) token: Token<'a>,
    pub(crate) lex: TaskIdLex,
}

/// Whether a token may sit between a task ID and the line end: schedule,
/// priority, and ID candidates form the trailing marker run. Anything else
/// (prose, routes, selectors, completions) ends the run.
fn is_trailing_id_run_member(token: &str) -> bool {
    lex_project_task_id(token).is_some()
        || super::markers::parse_schedule_token(token).is_some()
        || super::markers::parse_priority_token(token).is_some()
        || super::markers::parse_retired_priority_token(token).is_some()
}

/// Whether a child line seals the item as a project note: its last body
/// token parses as a project-note route marker (`@route^id@` /
/// `@route:id@` and named seals). The whole item takes the trailing-run
/// ID rule then, even when the parent line carries no marker itself.
pub(crate) fn child_line_seals_project_note(body: &str) -> bool {
    body.split_whitespace().last().is_some_and(|last| {
        super::tokens::parse_terminal_route_token(last)
            .ok()
            .flatten()
            .is_some_and(|route| {
                matches!(route.kind, CaptureKind::ProjectNote { .. })
            })
    })
}

/// Remove one trailing-run task ID token from `tokens`: the last ID-lex
/// token in the trailing run. A trailing route marker belongs to the item,
/// not the ID run, so it is set aside before the scan (a project note's
/// trailing `@route` marker); a lone `:`/`^` counts only when nothing but
/// that marker follows it, and a mid-run lone sigil stays literal text.
pub(crate) fn strip_trailing_task_id<'a>(
    tokens: &mut Vec<Token<'a>>,
) -> Option<StrippedTaskId<'a>> {
    let mut run_end = tokens.len();
    if run_end > 0
        && super::tokens::parse_terminal_route_token(tokens[run_end - 1].text)
            .ok()
            .flatten()
            .is_some()
    {
        run_end -= 1;
    }
    let mut run_start = run_end;
    while run_start > 0 && is_trailing_id_run_member(tokens[run_start - 1].text)
    {
        run_start -= 1;
    }
    let mut found: Option<usize> = None;
    for (offset, token) in tokens
        .iter()
        .enumerate()
        .skip(run_start)
        .take(run_end - run_start)
    {
        if lex_project_task_id(token.text).is_some() {
            found = Some(offset);
        }
    }
    let index = found?;
    let lex = lex_project_task_id(tokens[index].text)?;
    if matches!(lex, TaskIdLex::Lone { .. }) && index + 1 != run_end {
        return None;
    }
    let token = tokens.remove(index);
    Some(StrippedTaskId { token, lex })
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

pub(crate) fn unfinished_project_task_id_error(
    sigil: char,
    owner: &str,
) -> String {
    unfinished_task_id_error(sigil, owner)
}

/// A lone ` :` / ` ^` ending a list bullet or one of its nested lines: the
/// unfinished ID names its owner task.
pub(crate) fn unfinished_list_task_id_error(
    sigil: char,
    owner: &str,
) -> String {
    unfinished_task_id_error(sigil, owner)
}

fn unfinished_task_id_error(sigil: char, owner: &str) -> String {
    let suggestion =
        crate::native::capture_block_ids::suggest_ids(owner, sigil, &[])
            .into_iter()
            .next()
            .unwrap_or_else(|| "<block-id>".to_string());
    format!(
        "` {sigil}` on `{owner}` needs a task ID: end the bullet with ` {sigil}{suggestion}`"
    )
}

/// A complete ` :id` / ` ^id` on a nested list line: IDs live on the
/// owner's own bullet. `owner_bullet` is the owner's stripped body.
pub(crate) fn misplaced_list_task_id_error(
    token: &str,
    owner_bullet: &str,
) -> String {
    format!(
        "a task ID goes at the end of its task's own bullet: move `{token}` to `- {owner_bullet}`"
    )
}

/// A header with no bullets captures nothing.
pub(crate) fn empty_list_error(header: &str, notes: bool) -> String {
    if notes {
        format!(
            "`{header}` heads a list but has no bullets: add `- <note>` lines below it, or put the note on the same line (`Idea text {header}`)"
        )
    } else {
        format!(
            "`{header}` heads a list but has no bullets: add `- <task>` lines below it, or put the task on the same line (`Call the bank {header}`)"
        )
    }
}

/// A route marker plus item-wide markers on the header line: the header
/// stands alone. `bullet` is the first bullet's body for the example.
pub(crate) fn list_header_marker_error(
    header: &str,
    marker: &str,
    bullet: &str,
) -> String {
    format!(
        "`{header}` heads a list, so it stands alone on its line: move `{marker}` to the bullet it configures (`- {bullet} {marker}`)"
    )
}

/// A route-changing marker on a list bullet.
pub(crate) fn list_bullet_marker_error(
    header: &str,
    file: &str,
    marker: &str,
    suggestion: &str,
) -> String {
    format!(
        "every bullet under `{header}` files into {file}; `{marker}` would route this one elsewhere, so {suggestion}"
    )
}

/// The `+`/`%n`/`%l`/`%t`/`!note:id` variants name their own item form.
pub(crate) fn list_bullet_marker_suggestion(
    marker: &str,
    body: &str,
    route: &str,
) -> String {
    if marker.starts_with('+') {
        format!(
            "give it its own item (a blank line, then `{body} @{route}{marker}`)"
        )
    } else if marker == "%n" {
        "give the Pomodoro note its own item (a blank line, then `%n`)"
            .to_string()
    } else if marker == "%l" {
        "capture the link on its own (a blank line, then `%l` with the URL)"
            .to_string()
    } else if marker == "%t" {
        "capture the template on its own (a blank line, then the `%t` item)"
            .to_string()
    } else if marker.starts_with('!') {
        format!(
            "complete the existing task on its own (a blank line, then `{marker}`)"
        )
    } else {
        format!("give it its own item (a blank line, then `{body} {marker}`)")
    }
}

/// A non-open checkbox on a list bullet: list bullets become new open tasks.
pub(crate) fn list_bullet_checkbox_error(status: char) -> String {
    format!(
        "list bullets become new open tasks: drop `[{status}]` (complete an existing task with `!note:id`)"
    )
}

/// A bullet-start `#` under a list header: first-level `#` never types.
/// `example` shows the bullet's own words reordered trailing.
pub(crate) fn drawer_bullet_position_list_error(
    notes: bool,
    example: &str,
) -> String {
    if notes {
        format!(
            "list bullets are notes, so `#` cannot start one: type the note at the end of its line (`{example}`), or write `\\#` to keep a literal `#`"
        )
    } else {
        format!(
            "list bullets are tasks, so `#` cannot start one: type the task at the end of its line (`{example}`), or write `\\#` to keep a literal `#`"
        )
    }
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
    /// An accepted ID. The caller already stripped the token before
    /// marker extraction, so the body no longer holds it.
    Accept { sigil: char, id: String },
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

    /// Step 3: one child in source order. The caller strips the child's
    /// trailing-run task ID before marker extraction, so schedule and
    /// priority markers behind the ID still resolve; `stripped` carries
    /// that token (or `None` when the line holds no trailing-run ID).
    pub(crate) fn check_child(
        &mut self,
        stripped: Option<StrippedTaskId<'_>>,
        body: &str,
        depth: AuthoredDepth,
        line_number: usize,
    ) -> ChildTaskOutcome {
        let Some(stripped) = stripped else {
            return ChildTaskOutcome::Ignore;
        };
        let token = stripped.token.text;
        match stripped.lex {
            TaskIdLex::Lone { sigil } => {
                // A lone sigil ends the owner's trailing run on nested
                // lines too: it opens the unfinished-ID prompt for the
                // owner task.
                ChildTaskOutcome::Unfinished { sigil }
            }
            TaskIdLex::Invalid { raw, .. } => ChildTaskOutcome::Error {
                code: "invalid_project_task_id",
                message: invalid_project_task_id_charset_error(&raw),
            },
            TaskIdLex::Valid { sigil, id } => {
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
                let (_, without_checkbox) =
                    split_leading_checkbox(body.trim_start());
                if without_checkbox.split_whitespace().next().is_none() {
                    return ChildTaskOutcome::Error {
                        code: "invalid_project_task_id",
                        message: empty_project_task_body_error(line_number),
                    };
                }
                let link = sigil == ':';
                if link {
                    let (checkbox, _) =
                        split_leading_checkbox(body.trim_start());
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
                ChildTaskOutcome::Accept { sigil, id }
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
