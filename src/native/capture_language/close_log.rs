//! Shared `=x` Work Log bullet lexer for Pomodoro closes.
//!
//! Both the execution parser (`bob capture`) and the editor parser
//! (`capture-parse`, completion, rewrite) lex a close's child bullet lines
//! through [`lex_close_log_bullets`], so a mistyped entry reports the same
//! message and byte range everywhere. Diagnostic text lives in [`markers`];
//! this module only decides which diagnostic applies and where it points.
//!
//! A whole-item close (`=x`/`=X` plus an optional selection, alone on its
//! parent line) takes Work Log bullets as its child lines, using exactly the
//! authored-bullet line rules. A first-level bullet is numbered when its
//! first whitespace-separated token is all ASCII digits; a leading number is
//! always the task number, so `- 2 bugs fixed` names task 2 and text that
//! starts with a number must number every bullet. Every other first-level
//! bullet is unnumbered and its whole body is entry text. Bullets are
//! numbered all or none: the first first-level bullet (placeholder rows
//! skipped) fixes the list's kind, a later bullet of the other kind fails,
//! and dangling bullets (`- 1`) only occur in numbered lists.
//!
//! Unnumbered bullets log in order to the close's worked tasks `W`: in
//! selection mode (`<N>` typed, including `=x0`, or a non-empty `*<P>`)
//! `W` is the lexical `sorted(<N> ∪ *<P> ∪ !<M>)` and positions resolve
//! here; otherwise positions stay `None` for `bob capture` to resolve
//! against the running session. The assignment rule
//! ([`assign_log_positions`]) is: no worked task is an error, one worked
//! task takes every bullet, otherwise bullet `i` logs to `W[i]`, and more
//! bullets than worked tasks is an error.
//!
//! A two-space nested bullet is a detail of the nearest preceding entry,
//! numbered or not, and its text is entirely literal. Loggability of
//! numbered bullets is purely lexical: with `<N>` typed (including `=x0`)
//! or `*<P>` present only the numbers in `<N>`, `*<P>`, or `!<M>` start
//! entries, otherwise every number `>= 1` starts one except those in
//! `~<K>`. Every backslash is literal.

use super::close_selection::*;
use super::draft::*;
use super::item::{is_session_chain_token, session_equals_token};
use super::line::*;
use super::markers::*;
use super::model::*;
use super::tokens::{parse_caret_link_token, parse_pomodoro_route_token};
use crate::native::capture_pomodoro_close::wikilink_tokens;

/// One lexed Work Log entry: the numbered Task Link plus its literal text,
/// its nested detail lines, and the absolute byte ranges of the index token,
/// the entry text (first text token start through last text token end), and
/// each detail (first token start through last token end). Unnumbered
/// bullets report `index: None` (positional, resolved at execution when the
/// close names no selection) and `index_range: None` (no index span).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogEntryLex {
    pub(crate) index: Option<u32>,
    pub(crate) text: String,
    pub(crate) details: Vec<String>,
    pub(crate) index_range: Option<(usize, usize)>,
    pub(crate) text_range: (usize, usize),
    pub(crate) detail_ranges: Vec<(usize, usize)>,
}

/// One dangling Work Log bullet: a first-level bullet whose body is only a
/// task number. The editor reports it as an editing state; execution rejects
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogDanglingBullet {
    pub(crate) index: u32,
    pub(crate) index_range: (usize, usize),
}

/// A fully lexed bullet list: every complete entry in typed order plus every
/// dangling bullet in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogLexed {
    pub(crate) entries: Vec<CloseLogEntryLex>,
    pub(crate) dangling: Vec<CloseLogDanglingBullet>,
}

/// One lexical failure with its absolute byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogError {
    pub(crate) message: String,
    pub(crate) range: (usize, usize),
}

/// `true` when `index` can start a Work Log entry under these lists.
/// Selection mode is active when `<N>` was typed (including `=x0`) or a
/// nonempty `*<P>` group is present: only `<N>`, `*<P>`, and `!<M>` start
/// entries. Otherwise every number starts one except those in `~<K>`.
fn is_loggable(
    index: u32,
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
    drop: &[u32],
) -> bool {
    if in_progress.is_some() || !park.is_empty() {
        in_progress.is_some_and(|list| list.contains(&index))
            || park.contains(&index)
            || complete.contains(&index)
    } else {
        !drop.contains(&index)
    }
}

/// The default task number for an inline Work Log entry: the first task the
/// close works. With `<N>` typed (including `=x0`) or `*<P>` present, the
/// smallest of `<N>` union `*<P>` union `!<M>`; otherwise the smallest `n`
/// at or above 1 not in `~<K>`. `None` when the close works no task (for
/// example `=x0`, `=x0~2`). Whenever task 1 can take an entry, the default
/// is 1; in every case where a default of 1 would always fail, the default
/// moves to the first task that can.
pub(crate) fn default_log_index(
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
    drop: &[u32],
) -> Option<u32> {
    if in_progress.is_some() || !park.is_empty() {
        in_progress
            .iter()
            .flat_map(|list| list.iter())
            .chain(park.iter())
            .chain(complete.iter())
            .min()
            .copied()
    } else {
        let mut candidate = 1u32;
        loop {
            if !drop.contains(&candidate) {
                return Some(candidate);
            }
            candidate = candidate.checked_add(1)?;
        }
    }
}

/// Build the `list it` and `complete it` suggestions for a not-worked index.
fn not_worked_suggestions(
    index: u32,
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
    drop: &[u32],
) -> (String, String) {
    let drop_suffix = if drop.is_empty() {
        String::new()
    } else {
        format!(
            "~{}",
            drop.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let complete_suffix = |list: &[u32]| {
        if list.is_empty() {
            String::new()
        } else {
            format!(
                "!{}",
                list.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    };
    let park_suffix = |list: &[u32]| {
        if list.is_empty() {
            String::new()
        } else {
            format!(
                "*{}",
                list.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    };
    match in_progress {
        Some(list) if list.is_empty() => {
            let mut with_complete: Vec<u32> = complete.to_vec();
            if !with_complete.contains(&index) {
                with_complete.push(index);
                with_complete.sort_unstable();
            }
            // With `=x0`, "list it" parks when a park selection exists,
            // otherwise it continues as `<N>`.
            if park.is_empty() {
                (
                    format!(
                        "=x{index}{}{}",
                        park_suffix(park),
                        complete_suffix(complete)
                    ),
                    format!(
                        "=x0{}{}{}",
                        park_suffix(park),
                        drop_suffix,
                        complete_suffix(&with_complete)
                    ),
                )
            } else {
                let mut parked: Vec<u32> = park.to_vec();
                if !parked.contains(&index) {
                    parked.push(index);
                    parked.sort_unstable();
                }
                (
                    format!(
                        "=x0{}{}{}",
                        park_suffix(&parked),
                        drop_suffix,
                        complete_suffix(complete)
                    ),
                    format!(
                        "=x0{}{}{}",
                        park_suffix(park),
                        drop_suffix,
                        complete_suffix(&with_complete)
                    ),
                )
            }
        }
        Some(list) => {
            let mut listed: Vec<u32> = list.to_vec();
            if !listed.contains(&index) {
                listed.push(index);
                listed.sort_unstable();
            }
            let listed_text = listed
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let mut completed: Vec<u32> = complete.to_vec();
            if !completed.contains(&index) {
                completed.push(index);
                completed.sort_unstable();
            }
            (
                format!(
                    "=x{listed_text}{}{}{drop_suffix}",
                    park_suffix(park),
                    complete_suffix(complete)
                ),
                format!(
                    "=x{listed_text}{}{}{drop_suffix}",
                    park_suffix(park),
                    complete_suffix(&completed)
                ),
            )
        }
        None if !park.is_empty() => {
            let mut parked: Vec<u32> = park.to_vec();
            if !parked.contains(&index) {
                parked.push(index);
                parked.sort_unstable();
            }
            let mut completed: Vec<u32> = complete.to_vec();
            if !completed.contains(&index) {
                completed.push(index);
                completed.sort_unstable();
            }
            (
                format!(
                    "=x{}{}{drop_suffix}",
                    park_suffix(&parked),
                    complete_suffix(complete)
                ),
                format!(
                    "=x{}{}{drop_suffix}",
                    park_suffix(park),
                    complete_suffix(&completed)
                ),
            )
        }
        None => (format!("=x{index}"), format!("=x!{index}")),
    }
}

/// Failure to assign unnumbered Work Log bullets to worked tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PositionalLogError {
    /// The close works no task, so no bullet has one to log to.
    NoWorkedTask,
    /// More bullets than worked tasks: `worked` is the worked-task count.
    TooManyBullets { worked: usize },
}

/// Assign `count` unnumbered Work Log bullets in typed order to `worked`
/// (ascending task numbers). One worked task takes every bullet; otherwise
/// bullet `i` logs to `worked[i]`, so a single bullet logs to the first
/// worked task. Shared by the lexer (lexical `worked`) and the runtime
/// (session `worked`).
pub(crate) fn assign_log_positions(
    worked: &[u32],
    count: usize,
) -> Result<Vec<u32>, PositionalLogError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if worked.is_empty() {
        return Err(PositionalLogError::NoWorkedTask);
    }
    if worked.len() == 1 {
        return Ok(vec![worked[0]; count]);
    }
    if count <= worked.len() {
        return Ok(worked[..count].to_vec());
    }
    Err(PositionalLogError::TooManyBullets {
        worked: worked.len(),
    })
}

/// The close's lexical worked tasks `W = sorted(<N> ∪ *<P> ∪ !<M>)` when a
/// selection is active (`<N>` typed, including `=x0`, or a non-empty
/// `*<P>`); `None` otherwise, when positions resolve against the running
/// session at execution.
pub(crate) fn lexical_worked_tasks(
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
) -> Option<Vec<u32>> {
    if in_progress.is_none() && park.is_empty() {
        return None;
    }
    let mut worked: Vec<u32> = in_progress
        .iter()
        .flat_map(|list| list.iter())
        .chain(park.iter())
        .chain(complete.iter())
        .copied()
        .collect();
    worked.sort_unstable();
    worked.dedup();
    Some(worked)
}

/// `true` when the bullet's first token is all ASCII digits, making it a
/// numbered bullet whose leading number is always a task number (`0`,
/// `007`, and overflow still report the index errors).
fn is_numbered_bullet_token(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// `true` when the close item's first first-level child bullet is
/// unnumbered. Placeholder rows are skipped; anything else (no children,
/// an invalid line, a nested-first list) reports `false`. The
/// inline-mixing suggestion uses it to avoid proposing a numbered bullet
/// that would itself fail as mixed numbering.
pub(crate) fn close_first_bullet_is_unnumbered(
    child_lines: &[ItemLine<'_>],
) -> bool {
    for line in child_lines {
        match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => return false,
            AuthoredLineClass::Item(authored) => {
                if authored.depth != AuthoredDepth::First {
                    return false;
                }
                let tokens = body_tokens(authored.body, authored.body_start);
                let Some(first) = tokens.first() else {
                    return false;
                };
                return !is_numbered_bullet_token(first.text);
            }
        }
    }
    false
}

/// Tokenize a bullet body with absolute byte ranges. `body` is the text after
/// the bullet marker and `body_start` is its absolute start offset.
fn body_tokens(body: &str, body_start: usize) -> Vec<Token<'_>> {
    tokenize_with_spans(body)
        .into_iter()
        .map(|token| Token {
            text: token.text,
            start: token.start + body_start,
            end: token.end + body_start,
        })
        .collect()
}

/// Validate one bullet's literal text words: fences first, then block links.
/// Returns the whitespace-normalized text.
fn check_bullet_text(words: &[Token<'_>]) -> Result<String, CloseLogError> {
    let Some(first) = words.first() else {
        return Ok(String::new());
    };
    if first.text.starts_with("```") || first.text.starts_with("~~~") {
        return Err(CloseLogError {
            message: close_log_fence_error(),
            range: (first.start, first.end),
        });
    }
    for word in words {
        let hits = wikilink_tokens(word.text);
        if let Some(hit) = hits.first() {
            let link = word
                .text
                .get(
                    hit.start.min(word.text.len())
                        ..hit.end.min(word.text.len()),
                )
                .unwrap_or(word.text);
            // `wikilink_tokens` excludes a leading `!` from `token` but
            // includes it in the range; show what the user typed.
            let display = if word.text.as_bytes().get(hit.start) == Some(&b'!')
            {
                link.to_string()
            } else {
                hit.token.clone()
            };
            let display = if display.is_empty() {
                link.to_string()
            } else {
                display
            };
            return Err(CloseLogError {
                message: close_log_block_link_error(&display),
                range: (word.start, word.end),
            });
        }
    }
    Ok(words
        .iter()
        .map(|word| word.text)
        .collect::<Vec<_>>()
        .join(" "))
}

/// Parse one first-level bullet's index token. Returns the index on success.
fn parse_bullet_index(token: &Token<'_>) -> Result<u32, CloseLogError> {
    let text = token.text;
    if text == "0" || (text.len() > 1 && text.starts_with('0')) {
        return Err(CloseLogError {
            message: close_selection_starts_at_one_error(),
            range: (token.start, token.end),
        });
    }
    match text.parse::<u32>() {
        Ok(number) => Ok(number),
        Err(_) => Err(CloseLogError {
            message: close_selection_too_large_error(text),
            range: (token.start, token.end),
        }),
    }
}

/// Check one parsed index against the lexed selection lists.
fn check_index_loggable(
    index: u32,
    index_range: (usize, usize),
    close_token: &str,
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
    drop: &[u32],
) -> Result<(), CloseLogError> {
    if is_loggable(index, in_progress, park, complete, drop) {
        return Ok(());
    }
    if drop.contains(&index) {
        let drop_text = format!(
            "~{}",
            drop.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        return Err(CloseLogError {
            message: close_log_dropped_error(index, &drop_text),
            range: index_range,
        });
    }
    let (listed, completed) =
        not_worked_suggestions(index, in_progress, park, complete, drop);
    Err(CloseLogError {
        message: close_log_not_worked_error(
            index,
            close_token,
            &listed,
            &completed,
        ),
        range: index_range,
    })
}

/// One lexed inline Work Log entry on the close line itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseInlineLex {
    /// A complete entry: the resolved task index, its range when typed
    /// (`None` for the default number, which gets no index span), the
    /// whitespace-normalized text, and the text byte range.
    Entry {
        index: u32,
        index_range: Option<(usize, usize)>,
        text: String,
        text_range: (usize, usize),
    },
    /// A dangling number: the entry holds only a task number so far.
    Dangling {
        index: u32,
        index_range: (usize, usize),
    },
}

fn is_plain_positive_int(text: &str) -> bool {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if text == "0" {
        return false;
    }
    !(text.len() > 1 && text.starts_with('0'))
}

fn is_task_link_token(text: &str) -> bool {
    if text.starts_with('^') {
        parse_caret_link_token(text).is_ok()
    } else if text.starts_with('@') && text.contains(':') {
        parse_pomodoro_route_token(text).is_ok()
    } else {
        false
    }
}

/// Validate inline entry literal text: fences first, then block links.
/// Returns the whitespace-normalized text. Messages say "entry", not
/// "bullet".
fn check_inline_text(words: &[Token<'_>]) -> Result<String, CloseLogError> {
    let Some(first) = words.first() else {
        return Ok(String::new());
    };
    if first.text.starts_with("```") || first.text.starts_with("~~~") {
        return Err(CloseLogError {
            message: close_entry_fence_error(),
            range: (first.start, first.end),
        });
    }
    for word in words {
        let hits = wikilink_tokens(word.text);
        if let Some(hit) = hits.first() {
            let link = word
                .text
                .get(
                    hit.start.min(word.text.len())
                        ..hit.end.min(word.text.len()),
                )
                .unwrap_or(word.text);
            let display = if word.text.as_bytes().get(hit.start) == Some(&b'!')
            {
                link.to_string()
            } else {
                hit.token.clone()
            };
            let display = if display.is_empty() {
                link.to_string()
            } else {
                display
            };
            return Err(CloseLogError {
                message: close_entry_block_link_error(&display),
                range: (word.start, word.end),
            });
        }
    }
    Ok(words
        .iter()
        .map(|word| word.text)
        .collect::<Vec<_>>()
        .join(" "))
}

fn inline_entry_joined(tokens: &[Token<'_>]) -> String {
    tokens
        .iter()
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lex one inline Work Log entry: the whitespace tokens after the close
/// token on its line (before any trailing session-operator chain).
/// `close_token` is the display token (`=x`, `=x1,3,4`) interpolated into
/// diagnostics; the lists are the lexed selection. Checks run in the
/// plan's order; the first failure wins. Loggability is checked before
/// dangling, as for bullets.
pub(crate) fn lex_close_inline_entry(
    entry_tokens: &[Token<'_>],
    close_token: &str,
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
    drop: &[u32],
) -> Result<CloseInlineLex, CloseLogError> {
    let Some(first) = entry_tokens.first() else {
        unreachable!("inline entry tokens are never empty");
    };
    // 3. A leading start or close token belongs at the end of the line.
    if session_equals_token(first.text).is_some() {
        let leading_count = entry_tokens
            .iter()
            .take_while(|token| is_session_chain_token(token.text))
            .count()
            .max(1);
        let (leading, rest) = entry_tokens.split_at(leading_count);
        let leading_text = inline_entry_joined(leading);
        let fixed = if rest.is_empty() {
            format!("{close_token} {leading_text}")
        } else {
            format!(
                "{close_token} {} {leading_text}",
                inline_entry_joined(rest)
            )
        };
        let range = (
            leading
                .first()
                .map(|token| token.start)
                .unwrap_or(first.start),
            leading.last().map(|token| token.end).unwrap_or(first.end),
        );
        return Err(CloseLogError {
            message: close_inline_reorder_error(&fixed),
            range,
        });
    }
    // 4. A lone marker before the entry is stray (`-`, `+`, `*`, `--`, `++`).
    // `-2` and `+1` are entry text, never stray.
    if matches!(first.text, "-" | "+" | "*" | "--" | "++") {
        let rest = &entry_tokens[1..];
        let fixed = if rest.is_empty() {
            close_token.to_string()
        } else {
            format!("{close_token} {}", inline_entry_joined(rest))
        };
        return Err(CloseLogError {
            message: close_inline_stray_error(first.text, &fixed),
            range: (first.start, first.end),
        });
    }
    // 5. No-spaces hint: the spaceless join lexes as a selection. A
    // plain-integer first token is always a task number, never this hint.
    if !is_plain_positive_int(first.text) {
        let nospace = format!("{close_token}{}", first.text);
        if whole_item_close_after_x(&nospace).is_some_and(|after_x| {
            lex_close_selection(after_x, 0, &nospace).is_ok()
        }) {
            let escape = if entry_tokens.len() > 1 {
                default_log_index(in_progress, park, complete, drop).map(
                    |default| {
                        format!(
                            "{close_token} {default} {}",
                            inline_entry_joined(entry_tokens)
                        )
                    },
                )
            } else {
                None
            };
            return Err(CloseLogError {
                message: close_inline_no_spaces_error(
                    &nospace,
                    first.text,
                    escape.as_deref(),
                ),
                range: (first.start, first.end),
            });
        }
    }
    // 6-7. An all-digit first token names the task. Invalid numbers report
    // the shared index errors; a valid number must be loggable (with the
    // escape clause for a mistyped leading number).
    if !first.text.is_empty()
        && first.text.bytes().all(|byte| byte.is_ascii_digit())
    {
        let index = parse_bullet_index(first)?;
        if let Err(mut error) = check_index_loggable(
            index,
            (first.start, first.end),
            close_token,
            in_progress,
            park,
            complete,
            drop,
        ) {
            if let Some(default) =
                default_log_index(in_progress, park, complete, drop)
            {
                let joined = inline_entry_joined(entry_tokens);
                error.message.push_str(&format!(
                    "; to log text that starts with `{}`, put the task number first: `{close_token} {default} {joined}`",
                    first.text
                ));
            }
            return Err(error);
        }
        let rest = &entry_tokens[1..];
        if rest.is_empty() {
            return Ok(CloseInlineLex::Dangling {
                index,
                index_range: (first.start, first.end),
            });
        }
        // 9. The entry never ends with a Task Link form.
        if let Some(last) = rest.last()
            && is_task_link_token(last.text)
        {
            return Err(CloseLogError {
                message: close_inline_task_link_error(last.text),
                range: (last.start, last.end),
            });
        }
        // 10. Fences and block links, with entry wording.
        let text = check_inline_text(rest)?;
        let text_range = (
            rest.first().map(|token| token.start).unwrap_or(first.end),
            rest.last().map(|token| token.end).unwrap_or(first.end),
        );
        return Ok(CloseInlineLex::Entry {
            index,
            index_range: Some((first.start, first.end)),
            text,
            text_range,
        });
    }
    // 8. No default: the close works no task.
    let Some(default) = default_log_index(in_progress, park, complete, drop)
    else {
        let (listed, completed) =
            not_worked_suggestions(1, in_progress, park, complete, drop);
        let range = (
            first.start,
            entry_tokens
                .last()
                .map(|token| token.end)
                .unwrap_or(first.end),
        );
        return Err(CloseLogError {
            message: close_inline_no_default_error(
                close_token,
                &listed,
                &completed,
            ),
            range,
        });
    };
    // 9. The entry never ends with a Task Link form.
    if let Some(last) = entry_tokens.last()
        && is_task_link_token(last.text)
    {
        return Err(CloseLogError {
            message: close_inline_task_link_error(last.text),
            range: (last.start, last.end),
        });
    }
    // 10. Fences and block links, with entry wording.
    let text = check_inline_text(entry_tokens)?;
    let text_range = (
        first.start,
        entry_tokens
            .last()
            .map(|token| token.end)
            .unwrap_or(first.end),
    );
    Ok(CloseInlineLex::Entry {
        index: default,
        index_range: None,
        text,
        text_range,
    })
}

/// Lex a close's Work Log bullets: the close item's child `ItemLine`s.
/// `close_token` is the display token (`=x`, `=x2`, `=x1*2!3`) interpolated
/// into not-worked diagnostics; `in_progress`/`park`/`complete`/`drop` are
/// the lexed selection lists.
///
/// Placeholder rows are skipped. Invalid and orphaned lines report the
/// existing authored-bullet messages. Bullets are checked top to bottom and
/// the first error wins, including the two mixed-numbering errors; any
/// error outranks a dangling bullet.
pub(crate) fn lex_close_log_bullets(
    child_lines: &[ItemLine<'_>],
    close_token: &str,
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
    drop: &[u32],
) -> Result<CloseLogLexed, CloseLogError> {
    let mut entries: Vec<CloseLogEntryLex> = Vec::new();
    let mut dangling: Vec<CloseLogDanglingBullet> = Vec::new();
    // Nearest preceding first-level bullet: `Some(entry_index)` for a
    // complete entry, `None` for a dangling bullet, with `seen_first_level`
    // tracking whether any first-level bullet precedes (for orphaned
    // detection). Details under a dangling bullet are validated and dropped:
    // the spec holds only complete entries.
    let mut seen_first_level = false;
    let mut last_entry: Option<Option<usize>> = None;
    // All or none: the first first-level bullet fixes the list's kind
    // (`true` = numbered). Unnumbered bullets never dangle: their whole
    // body is entry text, even a lone `- foo`.
    let mut numbered_list: Option<bool> = None;

    for line in child_lines {
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => {
                return Err(CloseLogError {
                    message: invalid_child_line_error(line.line_number),
                    range: (line.raw.start, line.raw.end),
                });
            }
            AuthoredLineClass::Item(authored) => authored,
        };
        if authored.depth == AuthoredDepth::Nested && !seen_first_level {
            return Err(CloseLogError {
                message: orphaned_nested_bullet_error(line.line_number),
                range: (line.raw.start, line.raw.end),
            });
        }
        let tokens = body_tokens(authored.body, authored.body_start);
        // The classifier guarantees non-empty bodies for items, but guard
        // against an all-whitespace body anyway.
        if tokens.is_empty() {
            continue;
        }
        if authored.depth == AuthoredDepth::Nested {
            let text = check_bullet_text(&tokens)?;
            let detail_range = (
                tokens
                    .first()
                    .map(|token| token.start)
                    .unwrap_or(authored.body_start),
                tokens
                    .last()
                    .map(|token| token.end)
                    .unwrap_or(authored.body_start),
            );
            match last_entry {
                Some(Some(entry_index)) => {
                    entries[entry_index].details.push(text);
                    entries[entry_index].detail_ranges.push(detail_range);
                }
                // Validated but dropped: the owning bullet is dangling, so
                // no complete entry carries it yet.
                Some(None) => {}
                None => {
                    return Err(CloseLogError {
                        message: orphaned_nested_bullet_error(line.line_number),
                        range: (line.raw.start, line.raw.end),
                    });
                }
            }
            continue;
        }
        // First-level bullet: all or none. The first one fixes the kind;
        // a later bullet of the other kind fails before any index or
        // loggability check on that bullet.
        seen_first_level = true;
        let first = &tokens[0];
        let numbered = is_numbered_bullet_token(first.text);
        match numbered_list {
            Some(kind) if kind != numbered => {
                if kind {
                    let normalized = normalize_task_text(authored.body);
                    let smallest =
                        default_log_index(in_progress, park, complete, drop)
                            .unwrap_or(1);
                    let example = if normalized.is_empty() {
                        format!("- {smallest}")
                    } else {
                        format!("- {smallest} {normalized}")
                    };
                    return Err(CloseLogError {
                        message: close_log_missing_number_error(&example),
                        range: (first.start, first.end),
                    });
                }
                let default =
                    default_log_index(in_progress, park, complete, drop)
                        .unwrap_or(1);
                let body = normalize_task_text(authored.body);
                let suggestion = format!("- {default} {body}");
                return Err(CloseLogError {
                    message: close_log_mixed_number_error(
                        first.text,
                        &suggestion,
                    ),
                    range: (first.start, first.end),
                });
            }
            _ => {}
        }
        numbered_list = Some(numbered);
        if !numbered {
            let text = check_bullet_text(&tokens)?;
            let text_range = (
                first.start,
                tokens.last().map(|token| token.end).unwrap_or(first.end),
            );
            entries.push(CloseLogEntryLex {
                index: None,
                text,
                details: Vec::new(),
                index_range: None,
                text_range,
                detail_ranges: Vec::new(),
            });
            last_entry = Some(Some(entries.len() - 1));
            continue;
        }
        let index = parse_bullet_index(first)?;
        check_index_loggable(
            index,
            (first.start, first.end),
            close_token,
            in_progress,
            park,
            complete,
            drop,
        )?;
        if tokens.len() == 1 {
            dangling.push(CloseLogDanglingBullet {
                index,
                index_range: (first.start, first.end),
            });
            last_entry = Some(None);
            continue;
        }
        let text = check_bullet_text(&tokens[1..])?;
        let text_range = (
            tokens[1].start,
            tokens.last().map(|token| token.end).unwrap_or(first.end),
        );
        entries.push(CloseLogEntryLex {
            index: Some(index),
            text,
            details: Vec::new(),
            index_range: Some((first.start, first.end)),
            text_range,
            detail_ranges: Vec::new(),
        });
        last_entry = Some(Some(entries.len() - 1));
    }

    // An unnumbered list resolves positions lexically in selection mode;
    // otherwise every index stays `None` for execution to resolve against
    // the running session.
    if numbered_list == Some(false)
        && !entries.is_empty()
        && let Some(worked) = lexical_worked_tasks(in_progress, park, complete)
    {
        match assign_log_positions(&worked, entries.len()) {
            Ok(resolved) => {
                for (entry, index) in entries.iter_mut().zip(resolved) {
                    entry.index = Some(index);
                }
            }
            Err(PositionalLogError::NoWorkedTask) => {
                let (listed, completed) = not_worked_suggestions(
                    1,
                    in_progress,
                    park,
                    complete,
                    drop,
                );
                let range = entries
                    .first()
                    .map(|entry| entry.text_range)
                    .unwrap_or((0, 0));
                return Err(CloseLogError {
                    message: close_log_positional_none_error(
                        close_token,
                        &listed,
                        &completed,
                    ),
                    range,
                });
            }
            Err(PositionalLogError::TooManyBullets { .. }) => {
                let excess = &entries[worked.len()];
                let suggestion = format!("- {} {}", worked[0], excess.text);
                return Err(CloseLogError {
                    message: close_log_positional_too_many_error(
                        close_token,
                        &worked,
                        entries.len(),
                        &suggestion,
                    ),
                    range: excess.text_range,
                });
            }
        }
    }

    Ok(CloseLogLexed { entries, dangling })
}

/// Convert lexed entries into the execution/editor model entries.
/// Numbered entries use [`CloseLogOrigin::Bullet`]; unnumbered entries use
/// [`CloseLogOrigin::PositionalBullet`] with their 1-based typed position.
pub(crate) fn log_entries_from_lex(
    entries: &[CloseLogEntryLex],
) -> Vec<CloseLogEntry> {
    entries
        .iter()
        .enumerate()
        .map(|(ordinal, entry)| {
            let origin = match entry.index {
                Some(_) => CloseLogOrigin::Bullet,
                None => CloseLogOrigin::PositionalBullet {
                    position: ordinal as u32 + 1,
                },
            };
            CloseLogEntry {
                index: entry.index,
                text: entry.text.clone(),
                details: entry.details.clone(),
                origin,
            }
        })
        .collect()
}

/// Convert one lexed inline entry into the execution/editor model entry.
/// `close_token` is the typed close token; `default_index` is the lexical
/// default used for execution escapes.
pub(crate) fn log_entry_from_inline(
    lexed: &CloseInlineLex,
    close_token: &str,
    default_index: Option<u32>,
) -> CloseLogEntry {
    match lexed {
        CloseInlineLex::Entry {
            index,
            index_range,
            text,
            ..
        } => CloseLogEntry {
            index: Some(*index),
            text: text.clone(),
            details: Vec::new(),
            origin: CloseLogOrigin::Inline {
                close_token: close_token.to_string(),
                explicit: index_range.is_some(),
                default_index,
            },
        },
        CloseInlineLex::Dangling { .. } => {
            unreachable!("dangling inline entries never become specs")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_lists(
        close: &str,
    ) -> (Option<Vec<u32>>, Vec<u32>, Vec<u32>, Vec<u32>) {
        let after_x = whole_item_close_after_x(close).unwrap_or("");
        let base = close.len() - after_x.len();
        match lex_close_selection(after_x, base, close)
            .expect("valid selection")
        {
            CloseSelectionOutcome::Valid(lex) => {
                (lex.in_progress, lex.park, lex.complete, lex.drop)
            }
            CloseSelectionOutcome::Incomplete(_) => {
                panic!("{close}: expected valid selection")
            }
        }
    }

    fn lex_bullets(
        close: &str,
        draft: &str,
    ) -> Result<CloseLogLexed, CloseLogError> {
        let lines = split_physical_lines(draft);
        let item_lines: Vec<ItemLine<'_>> = lines
            .into_iter()
            .enumerate()
            .map(|(index, raw)| ItemLine {
                raw,
                line_number: index + 1,
            })
            .collect();
        let (in_progress, park, complete, drop) = selection_lists(close);
        lex_close_log_bullets(
            &item_lines[1..],
            close,
            in_progress.as_deref(),
            &park,
            &complete,
            &drop,
        )
    }

    fn valid_entries(
        close: &str,
        draft: &str,
    ) -> Vec<(Option<u32>, String, Vec<String>)> {
        match lex_bullets(close, draft).expect("valid bullets") {
            CloseLogLexed { entries, dangling } => {
                assert!(dangling.is_empty(), "unexpected dangling");
                entries
                    .into_iter()
                    .map(|entry| (entry.index, entry.text, entry.details))
                    .collect()
            }
        }
    }

    #[test]
    fn lexes_entries_and_details() {
        assert_eq!(
            valid_entries("=x", "=x\n- 1 wired the lexer"),
            vec![(Some(1), "wired the lexer".to_string(), Vec::new())]
        );
        assert_eq!(
            valid_entries("=x1,2", "=x1,2\n- 1 wired the lexer\n  - chose a hand-rolled lexer\n- 2 sketched the URL parser"),
            vec![
                (
                    Some(1),
                    "wired the lexer".to_string(),
                    vec!["chose a hand-rolled lexer".to_string()]
                ),
                (Some(2), "sketched the URL parser".to_string(), Vec::new()),
            ]
        );
        // Only the first token is an index: later numbers stay text.
        assert_eq!(
            valid_entries("=x1", "=x1\n- 1 fixed 3 bugs"),
            vec![(Some(1), "fixed 3 bugs".to_string(), Vec::new())]
        );
        // Markers stay literal in bullets.
        assert_eq!(
            valid_entries("=x", "=x\n- 1 moved @@inbox s:3"),
            vec![(Some(1), "moved @@inbox s:3".to_string(), Vec::new())]
        );
        // Backslashes stay literal: no escape.
        assert_eq!(
            valid_entries("=x", "=x\n- 1 fixed \\3 bugs"),
            vec![(Some(1), "fixed \\3 bugs".to_string(), Vec::new())]
        );
        // Placeholders are skipped.
        assert_eq!(
            valid_entries("=x", "=x\n- 1 wired the lexer\n- "),
            vec![(Some(1), "wired the lexer".to_string(), Vec::new())]
        );
    }

    #[test]
    fn rejects_bad_bullets() {
        // A numbered list rejects a later unnumbered bullet.
        let missing = lex_bullets("=x", "=x\n- 1 ok\n- wired the lexer")
            .expect_err("missing number");
        assert!(
            missing.message.contains("start each Work Log bullet"),
            "{}",
            missing.message
        );
        assert!(
            missing.message.contains("numbered all or none"),
            "{}",
            missing.message
        );
        let not_worked =
            lex_bullets("=x1", "=x1\n- 2 foo").expect_err("not worked");
        assert!(
            not_worked.message.contains("isn't worked by `=x1`"),
            "{}",
            not_worked.message
        );
        let dropped =
            lex_bullets("=x~2", "=x~2\n- 2 foo").expect_err("dropped");
        assert!(
            dropped.message.contains("is dropped by `~2`"),
            "{}",
            dropped.message
        );
        let zero = lex_bullets("=x", "=x\n- 0 foo").expect_err("zero");
        assert!(zero.message.contains("start at 1"), "{}", zero.message);
        let block = lex_bullets("=x", "=x\n- 1 see [[bob#^web-capture]]")
            .expect_err("block link");
        assert!(
            block.message.contains("can't contain the block link"),
            "{}",
            block.message
        );
        let fence = lex_bullets("=x", "=x\n- 1 ```rust").expect_err("fence");
        assert!(
            fence.message.contains("can't start with a code fence"),
            "{}",
            fence.message
        );
        let detail_block = lex_bullets("=x", "=x\n- 1 ok\n  - ![[bob#^x]]")
            .expect_err("detail block");
        assert!(
            detail_block
                .message
                .contains("can't contain the block link"),
            "{}",
            detail_block.message
        );
    }

    #[test]
    fn reports_dangling() {
        match lex_bullets("=x", "=x\n- 1").expect("dangling") {
            CloseLogLexed { entries, dangling } => {
                assert!(entries.is_empty());
                assert_eq!(dangling.len(), 1);
                assert_eq!(dangling[0].index, 1);
            }
        }
    }

    fn lex_inline(
        close: &str,
        entry: &str,
    ) -> Result<CloseInlineLex, CloseLogError> {
        let (in_progress, park, complete, drop) = selection_lists(close);
        let tokens = tokenize_with_spans(entry);
        lex_close_inline_entry(
            &tokens,
            close,
            in_progress.as_deref(),
            &park,
            &complete,
            &drop,
        )
    }

    fn inline_entry(
        close: &str,
        entry: &str,
    ) -> (u32, Option<(usize, usize)>, String) {
        match lex_inline(close, entry).expect("valid inline entry") {
            CloseInlineLex::Entry {
                index,
                index_range,
                text,
                ..
            } => (index, index_range, text),
            CloseInlineLex::Dangling { .. } => {
                panic!("{close} {entry}: dangling")
            }
        }
    }

    #[test]
    fn default_log_index_covers_every_close_shape() {
        for (close, expected) in [
            ("=x", Some(1)),
            ("=x1,3,4", Some(1)),
            ("=x2", Some(2)),
            ("=x3,4", Some(3)),
            ("=x*2", Some(2)),
            ("=x0!2", Some(2)),
            ("=x~1", Some(2)),
            ("=x!2", Some(1)),
            ("=x0", None),
            ("=x0~2", None),
        ] {
            let (in_progress, park, complete, drop) = selection_lists(close);
            assert_eq!(
                default_log_index(
                    in_progress.as_deref(),
                    &park,
                    &complete,
                    &drop
                ),
                expected,
                "{close}"
            );
        }
        // The bullet missing-number example now follows drops too.
        match lex_bullets("=x~1", "=x~1\n- 2 ok\n- wired").expect_err("missing")
        {
            error => assert!(
                error.message.contains("- 2 wired"),
                "{}",
                error.message
            ),
        }
    }

    #[test]
    fn inline_entry_defaults_and_leading_numbers() {
        assert_eq!(
            inline_entry("=x", "wired the lexer"),
            (1, None, "wired the lexer".to_string())
        );
        assert_eq!(
            inline_entry("=x1,3,4", "3 boom"),
            (3, Some((0, 1)), "boom".to_string())
        );
        // Only the first token is an index; later numbers stay text.
        assert_eq!(
            inline_entry("=x", "1 3 bugs fixed"),
            (1, Some((0, 1)), "3 bugs fixed".to_string())
        );
        // Markers stay literal; wikilinks are allowed.
        assert_eq!(
            inline_entry("=x", "moved @@inbox s:3"),
            (1, None, "moved @@inbox s:3".to_string())
        );
        assert_eq!(
            inline_entry("=x", "see [[Design notes]]"),
            (1, None, "see [[Design notes]]".to_string())
        );
        // A trailing adjust token with no start/close before it stays text.
        assert_eq!(
            inline_entry("=x", "got a +1"),
            (1, None, "got a +1".to_string())
        );
        // Tabs and Unicode whitespace-normalize with exact ranges.
        match lex_inline("=x", "wired\tthé lexer").expect("unicode") {
            CloseInlineLex::Entry {
                text, text_range, ..
            } => {
                assert_eq!(text, "wired thé lexer");
                assert_eq!(text_range, (0, "wired\tthé lexer".len()));
            }
            CloseInlineLex::Dangling { .. } => panic!("dangling"),
        }
    }

    #[test]
    fn inline_entry_reports_diagnostics_in_order() {
        // Reorder: leading operators move to the end.
        let reorder = lex_inline("=x", "= wired").expect_err("reorder");
        assert!(
            reorder.message.contains("`=x wired =`"),
            "{}",
            reorder.message
        );
        assert_eq!(reorder.range, (0, 1));
        // Stray marker.
        let stray = lex_inline("=x", "- wired the lexer").expect_err("stray");
        assert!(
            stray.message.contains("drop the stray `-`"),
            "{}",
            stray.message
        );
        // No-spaces join with the escape.
        let nospace = lex_inline("=x", "!2 shipped").expect_err("no-spaces");
        assert!(nospace.message.contains("(`=x!2`)"), "{}", nospace.message);
        assert!(
            nospace.message.contains("`=x 1 !2 shipped`"),
            "{}",
            nospace.message
        );
        // Bad numbers (the spaceless join must not lex, else the
        // no-spaces hint wins first).
        let zero = lex_inline("=x", "00 foo").expect_err("zero");
        assert!(zero.message.contains("start at 1"), "{}", zero.message);
        // Not worked with the escape.
        let worked = lex_inline("=x1", "2 foo").expect_err("not worked");
        assert!(
            worked.message.contains("isn't worked by `=x1`"),
            "{}",
            worked.message
        );
        assert!(
            worked.message.contains("`=x1 1 2 foo`"),
            "{}",
            worked.message
        );
        // No default.
        let none = lex_inline("=x0", "wired it").expect_err("no default");
        assert!(none.message.contains("works no task"), "{}", none.message);
        // Task Link ending.
        let link = lex_inline("=x", "^bob:ready=").expect_err("link");
        assert!(
            link.message.contains("can't end with the Task Link"),
            "{}",
            link.message
        );
        // Block link and fence say entry, not bullet.
        let block =
            lex_inline("=x", "see [[bob#^web-capture]]").expect_err("block");
        assert!(
            block.message.contains("a Work Log entry can't contain"),
            "{}",
            block.message
        );
        let fence = lex_inline("=x", "```rust").expect_err("fence");
        assert!(
            fence.message.contains("a Work Log entry can't start"),
            "{}",
            fence.message
        );
    }

    #[test]
    fn inline_entry_reports_dangling() {
        match lex_inline("=x", "2").expect("dangling") {
            CloseInlineLex::Dangling { index, index_range } => {
                assert_eq!(index, 2);
                assert_eq!(index_range, (0, 1));
            }
            CloseInlineLex::Entry { .. } => panic!("entry"),
        }
        // Loggability wins over dangling, as for bullets.
        let bad = lex_inline("=x1", "2").expect_err("not worked");
        assert!(bad.message.contains("isn't worked by"), "{}", bad.message);
    }

    fn positional_entries(close: &str, draft: &str) -> Vec<CloseLogEntryLex> {
        let CloseLogLexed { entries, dangling } =
            lex_bullets(close, draft).expect("valid bullets");
        assert!(dangling.is_empty(), "unexpected dangling");
        entries
    }

    #[test]
    fn unnumbered_bullets_resolve_positionally_in_selection_mode() {
        // The user's example: identical to the numbered form, with no
        // index ranges.
        let entries =
            positional_entries("=x3,4", "=x3,4\n- foo bar\n- baz bam");
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.index, entry.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(Some(3), "foo bar"), (Some(4), "baz bam"),]
        );
        assert!(
            entries.iter().all(|entry| entry.index_range.is_none()),
            "no index spans"
        );
        // A single worked task takes every bullet.
        assert_eq!(
            positional_entries("=x2", "=x2\n- a\n- b")
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            vec![Some(2), Some(2)]
        );
        // Unions resolve in ascending order.
        assert_eq!(
            positional_entries("=x1!3", "=x1!3\n- a\n- b")
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(3)]
        );
        assert_eq!(
            positional_entries("=x1*2", "=x1*2\n- a\n- b")
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
        // A single bullet logs to the first worked task.
        assert_eq!(
            positional_entries("=x3,4", "=x3,4\n- a")
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            vec![Some(3)]
        );
        // Details attach to positional entries.
        let entries = positional_entries(
            "=x1,2",
            "=x1,2\n- foo\n  - chose a lexer\n- bar",
        );
        assert_eq!(entries[0].index, Some(1));
        assert_eq!(entries[0].details, vec!["chose a lexer".to_string()]);
        assert_eq!(entries[1].index, Some(2));
        // A lone leading number is always a task number.
        let entries = positional_entries("=x", "=x\n- 2 bugs fixed");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index, Some(2));
        assert_eq!(entries[0].text, "bugs fixed");
    }

    #[test]
    fn unnumbered_bullets_stay_unresolved_without_a_selection() {
        // Plain `=x`, `=x!2`, and `=x~2` leave positions for execution.
        for close in ["=x", "=x!2", "=x~2"] {
            let entries = positional_entries(close, &format!("{close}\n- foo"));
            assert_eq!(entries.len(), 1, "{close}");
            assert_eq!(entries[0].index, None, "{close}");
            assert_eq!(entries[0].index_range, None, "{close}");
            assert_eq!(entries[0].text, "foo", "{close}");
        }
    }

    #[test]
    fn unnumbered_bullets_report_lexical_failures() {
        // Too many: the message names the close, the counts, and the fix;
        // the range covers the first excess bullet's text.
        let too_many =
            lex_bullets("=x3,4", "=x3,4\n- a\n- b\n- c").expect_err("too many");
        assert!(
            too_many.message.contains(
                "`=x3,4` works 2 tasks (3 and 4) but has 3 unnumbered Work Log bullets"
            ),
            "{}",
            too_many.message
        );
        assert!(too_many.message.contains("`- 3 c`"), "{}", too_many.message);
        assert_eq!(too_many.range, (16, 17));
        // None: `=x0` works no task.
        let none = lex_bullets("=x0", "=x0\n- a").expect_err("no worked task");
        assert!(
            none.message.contains(
                "`=x0` works no task, so its Work Log bullets have none to log to"
            ),
            "{}",
            none.message
        );
        assert!(none.message.contains("`=x1`"), "{}", none.message);
        assert!(none.message.contains("`=x0!1`"), "{}", none.message);
        assert_eq!(none.range, (6, 7));
    }

    #[test]
    fn mixed_numbering_fails_both_orders() {
        // Numbered list, later unnumbered bullet: the missing-number error
        // with the all-or-none rule.
        let missing = lex_bullets("=x", "=x\n- 1 a\n- b").expect_err("missing");
        assert!(
            missing.message.contains("start each Work Log bullet"),
            "{}",
            missing.message
        );
        assert!(
            missing.message.contains("numbered all or none"),
            "{}",
            missing.message
        );
        assert_eq!(missing.range, (11, 12));
        // Unnumbered list, later numbered bullet: the mixed-numbering
        // error on the index token, before any index or loggability check.
        let mixed = lex_bullets("=x", "=x\n- a\n- 2 b").expect_err("mixed");
        assert!(
            mixed.message.contains("numbered all or none"),
            "{}",
            mixed.message
        );
        assert!(
            mixed.message.contains("to log text that starts with `2`"),
            "{}",
            mixed.message
        );
        assert!(mixed.message.contains("`- 1 2 b`"), "{}", mixed.message);
        assert_eq!(mixed.range, (9, 10));
        // The mixed check wins over a bad number on that bullet.
        let zero = lex_bullets("=x", "=x\n- a\n- 0 b").expect_err("mixed zero");
        assert!(
            zero.message.contains("numbered all or none"),
            "{}",
            zero.message
        );
    }

    #[test]
    fn positional_entries_carry_their_typed_position() {
        let lexed =
            lex_bullets("=x", "=x\n- foo\n- bar").expect("valid bullets");
        let entries = log_entries_from_lex(&lexed.entries);
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.index, entry.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(None, "foo"), (None, "bar")]
        );
        assert_eq!(
            entries[0].origin,
            CloseLogOrigin::PositionalBullet { position: 1 }
        );
        assert_eq!(
            entries[1].origin,
            CloseLogOrigin::PositionalBullet { position: 2 }
        );
        // Numbered entries keep the bullet origin.
        let lexed = lex_bullets("=x1,2", "=x1,2\n- 1 foo\n- 2 bar")
            .expect("valid bullets");
        let entries = log_entries_from_lex(&lexed.entries);
        assert_eq!(entries[0].origin, CloseLogOrigin::Bullet);
        assert_eq!(
            entries.iter().map(|entry| entry.index).collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
    }

    #[test]
    fn assign_log_positions_follows_the_assignment_rule() {
        use super::PositionalLogError;
        assert_eq!(assign_log_positions(&[], 0), Ok(vec![]));
        assert_eq!(
            assign_log_positions(&[], 2),
            Err(PositionalLogError::NoWorkedTask)
        );
        assert_eq!(assign_log_positions(&[2], 2), Ok(vec![2, 2]));
        assert_eq!(assign_log_positions(&[1, 3], 2), Ok(vec![1, 3]));
        assert_eq!(assign_log_positions(&[1, 2], 1), Ok(vec![1]));
        assert_eq!(assign_log_positions(&[3, 4], 2), Ok(vec![3, 4]));
        assert_eq!(
            assign_log_positions(&[3, 4], 3),
            Err(PositionalLogError::TooManyBullets { worked: 2 })
        );
        assert_eq!(assign_log_positions(&[1, 2, 3], 3), Ok(vec![1, 2, 3]));
    }

    #[test]
    fn first_bullet_kind_detection_skips_placeholders() {
        use super::super::draft::split_physical_lines;
        use super::super::model::ItemLine;
        fn child_lines(draft: &str) -> Vec<ItemLine<'_>> {
            split_physical_lines(draft)
                .into_iter()
                .enumerate()
                .map(|(index, raw)| ItemLine {
                    raw,
                    line_number: index + 1,
                })
                .collect()
        }
        let lines = child_lines("=x\n- \n- foo");
        assert!(close_first_bullet_is_unnumbered(&lines[1..]));
        let lines = child_lines("=x\n- \n- 1 foo");
        assert!(!close_first_bullet_is_unnumbered(&lines[1..]));
        let lines = child_lines("=x");
        assert!(!close_first_bullet_is_unnumbered(&lines[1..]));
    }
}
