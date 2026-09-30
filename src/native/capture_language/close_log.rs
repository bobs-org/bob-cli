//! Shared `=x` Work Log tail lexer for Pomodoro closes.
//!
//! Both the execution parser (`bob capture`) and the editor parser
//! (`capture-parse`, completion, rewrite) lex a close tail through
//! [`lex_close_log_tail`], so a mistyped entry reports the same message and
//! byte range everywhere. Diagnostic text lives in [`markers`]; this module
//! only decides which diagnostic applies and where it points.
//!
//! The tail is everything after the whitespace that follows the close token,
//! already stripped of any trailing start run by `draft.rs`. Loggability is
//! purely lexical: with `<N>` typed (including `=x0`) only the numbers in
//! `<N>` or `!<M>` start entries, otherwise every number `>= 1` starts one
//! except those in `~<K>`. A `\` before a digit or `=` escapes it into text.

use super::markers::*;
use super::model::*;
use crate::native::capture_pomodoro_close::wikilink_tokens;

/// One lexed Work Log entry: the numbered Task Link plus its literal,
/// unescaped text and the absolute byte ranges of the index token and the
/// entry text (first text token start through last text token end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogEntryLex {
    pub(crate) index: u32,
    pub(crate) text: String,
    pub(crate) index_range: (usize, usize),
    pub(crate) text_range: (usize, usize),
}

/// A fully lexed tail: every entry in typed order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogValid {
    pub(crate) entries: Vec<CloseLogEntryLex>,
}

/// A tail ending in a dangling index: the complete entries plus the index
/// that still needs its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogDangling {
    pub(crate) entries: Vec<CloseLogEntryLex>,
    pub(crate) index: u32,
    pub(crate) index_range: (usize, usize),
}

/// The result of lexing a Work Log tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseLogOutcome {
    Valid(CloseLogValid),
    Dangling(CloseLogDangling),
}

/// One lexical failure with its absolute byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogError {
    pub(crate) message: String,
    pub(crate) range: (usize, usize),
}

/// Whether a tail token is an entry index or entry text. Leading-zero
/// numbers (`007`) and later `0` tokens are always text; overflow is always
/// an error.
enum TailTokenKind {
    Index(u32),
    Text(String),
}

/// Unescape one tail token: a leading `\` before a digit or `=` loses the
/// backslash and is always text. Returns `None` when the token is not
/// escaped.
fn unescape_tail_token(token: &str) -> Option<String> {
    let rest = token.strip_prefix('\\')?;
    let second = rest.as_bytes().first()?;
    if second.is_ascii_digit() || *second == b'=' {
        Some(rest.to_string())
    } else {
        None
    }
}

/// Classify one tail token. `is_first` selects the first-token error policy
/// for `0` and leading-zero numbers.
fn classify_tail_token(
    token: &Token<'_>,
    is_first: bool,
) -> Result<TailTokenKind, CloseLogError> {
    if let Some(unescaped) = unescape_tail_token(token.text) {
        return Ok(TailTokenKind::Text(unescaped));
    }
    let text = token.text;
    if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
        if text.len() > 1 && text.starts_with('0') {
            if is_first {
                return Err(CloseLogError {
                    message: close_selection_starts_at_one_error(),
                    range: (token.start, token.end),
                });
            }
            return Ok(TailTokenKind::Text(text.to_string()));
        }
        if text == "0" {
            if is_first {
                return Err(CloseLogError {
                    message: close_selection_starts_at_one_error(),
                    range: (token.start, token.end),
                });
            }
            return Ok(TailTokenKind::Text(text.to_string()));
        }
        match text.parse::<u32>() {
            Ok(number) => Ok(TailTokenKind::Index(number)),
            Err(_) => Err(CloseLogError {
                message: close_selection_too_large_error(text),
                range: (token.start, token.end),
            }),
        }
    } else {
        Ok(TailTokenKind::Text(text.to_string()))
    }
}

/// `true` when `index` can start a Work Log entry under these lists.
fn is_loggable(
    index: u32,
    in_progress: Option<&[u32]>,
    complete: &[u32],
    drop: &[u32],
) -> bool {
    if let Some(list) = in_progress {
        list.contains(&index) || complete.contains(&index)
    } else {
        !drop.contains(&index)
    }
}

/// Build the `list it` and `complete it` suggestions for a not-worked index.
fn not_worked_suggestions(
    index: u32,
    in_progress: Option<&[u32]>,
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
    match in_progress {
        Some(list) if list.is_empty() => {
            let mut with_complete: Vec<u32> = complete.to_vec();
            if !with_complete.contains(&index) {
                with_complete.push(index);
                with_complete.sort_unstable();
            }
            (
                format!("=x{index}{}", complete_suffix(complete)),
                format!("=x0{}{drop_suffix}", complete_suffix(&with_complete)),
            )
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
                    "=x{listed_text}{}{drop_suffix}",
                    complete_suffix(complete)
                ),
                format!(
                    "=x{listed_text}{}{drop_suffix}",
                    complete_suffix(&completed)
                ),
            )
        }
        None => (format!("=x{index}"), format!("=x!{index}")),
    }
}

/// Validate one flushed entry's text: fences first, then block links.
fn check_entry_text(
    words: &[(String, (usize, usize))],
) -> Result<String, CloseLogError> {
    let Some((first_word, first_range)) = words.first() else {
        return Ok(String::new());
    };
    if first_word.starts_with("```") || first_word.starts_with("~~~") {
        return Err(CloseLogError {
            message: close_log_fence_error(),
            range: *first_range,
        });
    }
    for (word, range) in words {
        let hits = wikilink_tokens(word);
        if let Some(hit) = hits.first() {
            let link = word
                .get(hit.start.min(word.len())..hit.end.min(word.len()))
                .unwrap_or(&word[..]);
            // `wikilink_tokens` excludes a leading `!` from `token` but
            // includes it in the range; show what the user typed.
            let display = if word.as_bytes().get(hit.start) == Some(&b'!') {
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
                range: *range,
            });
        }
    }
    Ok(words
        .iter()
        .map(|(word, _)| word.as_str())
        .collect::<Vec<_>>()
        .join(" "))
}

/// Lex a Work Log tail: the whitespace-separated tokens after the close
/// token, with absolute byte ranges. `close_token` is the display token
/// (`=x`, `=x2`, `=x1!2`) interpolated into not-worked diagnostics;
/// `in_progress`/`complete`/`drop` are the lexed selection lists.
pub(crate) fn lex_close_log_tail(
    tail_tokens: &[Token<'_>],
    close_token: &str,
    in_progress: Option<&[u32]>,
    complete: &[u32],
    drop: &[u32],
) -> Result<CloseLogOutcome, CloseLogError> {
    if tail_tokens.is_empty() {
        return Ok(CloseLogOutcome::Valid(CloseLogValid {
            entries: Vec::new(),
        }));
    }
    // The first tail token must be a loggable index.
    let first = &tail_tokens[0];
    if unescape_tail_token(first.text).is_some() {
        return Err(CloseLogError {
            message: close_log_tail_start_error(),
            range: (first.start, first.end),
        });
    }
    if first.text.bytes().all(|byte| byte.is_ascii_digit())
        && !first.text.is_empty()
    {
        match classify_tail_token(first, true)? {
            TailTokenKind::Index(number) => {
                if !is_loggable(number, in_progress, complete, drop) {
                    if drop.contains(&number) {
                        let drop_text = format!(
                            "~{}",
                            drop.iter()
                                .map(u32::to_string)
                                .collect::<Vec<_>>()
                                .join(",")
                        );
                        return Err(CloseLogError {
                            message: close_log_dropped_error(
                                number, &drop_text,
                            ),
                            range: (first.start, first.end),
                        });
                    }
                    let (listed, completed) = not_worked_suggestions(
                        number,
                        in_progress,
                        complete,
                        drop,
                    );
                    return Err(CloseLogError {
                        message: close_log_not_worked_error(
                            number,
                            close_token,
                            &listed,
                            &completed,
                        ),
                        range: (first.start, first.end),
                    });
                }
            }
            TailTokenKind::Text(_) => {
                return Err(CloseLogError {
                    message: close_log_tail_start_error(),
                    range: (first.start, first.end),
                });
            }
        }
    } else {
        return Err(CloseLogError {
            message: close_log_tail_start_error(),
            range: (first.start, first.end),
        });
    }

    let mut entries: Vec<CloseLogEntryLex> = Vec::new();
    let mut current_index: Option<(u32, (usize, usize))> = None;
    let mut current_text: Vec<(String, (usize, usize))> = Vec::new();

    for (position, token) in tail_tokens.iter().enumerate() {
        let is_first = position == 0;
        // A bare-digit token that parses and is loggable starts a new
        // entry; every other token (including non-loggable numbers, `0`,
        // leading-zero numbers, and escapes) is entry text. Overflow still
        // errors from the classifier.
        enum TailAction {
            Index(u32),
            Text(String),
        }
        let action = match classify_tail_token(token, is_first)? {
            TailTokenKind::Index(number)
                if is_loggable(number, in_progress, complete, drop) =>
            {
                TailAction::Index(number)
            }
            TailTokenKind::Index(number) => {
                // Non-loggable bare number inside the tail is literal text.
                // (The first token already reported dropped/not-worked, so
                // later positions stay text, e.g. the `3` in
                // `=x1 1 fixed 3 bugs`.)
                TailAction::Text(number.to_string())
            }
            TailTokenKind::Text(word) => TailAction::Text(word),
        };
        if let TailAction::Index(number) = action {
            if let Some((prev, _)) = current_index {
                if current_text.is_empty() {
                    return Err(CloseLogError {
                        message: close_log_empty_entry_error(prev, number),
                        range: (token.start, token.end),
                    });
                }
                let (prev_number, prev_range) =
                    current_index.take().expect("pending index");
                let text = check_entry_text(&current_text)?;
                let text_range = (
                    current_text
                        .first()
                        .map(|(_, range)| range.0)
                        .unwrap_or(prev_range.1),
                    current_text
                        .last()
                        .map(|(_, range)| range.1)
                        .unwrap_or(prev_range.1),
                );
                entries.push(CloseLogEntryLex {
                    index: prev_number,
                    text,
                    index_range: prev_range,
                    text_range,
                });
                current_text = Vec::new();
            }
            current_index = Some((number, (token.start, token.end)));
        } else {
            let TailAction::Text(word) = action else {
                unreachable!("index handled above");
            };
            // The first token is always an index (checked above), so text
            // here always belongs to a pending entry.
            if current_index.is_none() {
                return Err(CloseLogError {
                    message: close_log_tail_start_error(),
                    range: (token.start, token.end),
                });
            }
            current_text.push((word, (token.start, token.end)));
        }
    }

    let Some((number, index_range)) = current_index else {
        return Err(CloseLogError {
            message: close_log_tail_start_error(),
            range: (tail_tokens[0].start, tail_tokens[0].end),
        });
    };
    if current_text.is_empty() {
        return Ok(CloseLogOutcome::Dangling(CloseLogDangling {
            entries,
            index: number,
            index_range,
        }));
    }
    let text = check_entry_text(&current_text)?;
    let text_range = (
        current_text
            .first()
            .map(|(_, range)| range.0)
            .unwrap_or(index_range.1),
        current_text
            .last()
            .map(|(_, range)| range.1)
            .unwrap_or(index_range.1),
    );
    entries.push(CloseLogEntryLex {
        index: number,
        text,
        index_range,
        text_range,
    });
    Ok(CloseLogOutcome::Valid(CloseLogValid { entries }))
}

/// Convert lexed entries into the execution/editor model entries.
pub(crate) fn log_entries_from_lex(
    entries: &[CloseLogEntryLex],
) -> Vec<CloseLogEntry> {
    entries
        .iter()
        .map(|entry| CloseLogEntry {
            index: entry.index,
            text: entry.text.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::close_selection::*;
    use super::*;

    fn tail_tokens(text: &str) -> Vec<Token<'_>> {
        super::super::model::tokenize_with_spans(text)
            .into_iter()
            .map(|token| Token {
                text: token.text,
                start: token.start,
                end: token.end,
            })
            .collect()
    }

    fn lex_tail(
        close: &str,
        tail: &str,
    ) -> Result<CloseLogOutcome, CloseLogError> {
        let after_x = whole_item_close_after_x(close).unwrap_or("");
        let base = close.len() - after_x.len();
        let selection = match lex_close_selection(after_x, base, close)
            .expect("valid selection")
        {
            CloseSelectionOutcome::Valid(lex) => lex,
            CloseSelectionOutcome::Incomplete(_) => {
                panic!("{close}: expected valid selection")
            }
        };
        // Tail offsets sit after the close token plus one space.
        let offset = close.len() + 1;
        let raw_tokens = tail_tokens(tail);
        let tokens: Vec<Token<'_>> = raw_tokens
            .into_iter()
            .map(|token| Token {
                text: token.text,
                start: token.start + offset,
                end: token.end + offset,
            })
            .collect();
        lex_close_log_tail(
            &tokens,
            close,
            selection.in_progress.as_deref(),
            &selection.complete,
            &selection.drop,
        )
    }

    fn valid_entries(close: &str, tail: &str) -> Vec<(u32, String)> {
        match lex_tail(close, tail).expect("valid tail") {
            CloseLogOutcome::Valid(valid) => valid
                .entries
                .into_iter()
                .map(|entry| (entry.index, entry.text))
                .collect(),
            CloseLogOutcome::Dangling(dangling) => {
                panic!("{close} {tail}: dangling at {}", dangling.index)
            }
        }
    }

    #[test]
    fn lexes_the_worked_table() {
        assert_eq!(
            valid_entries("=x", "1 wired the lexer"),
            vec![(1, "wired the lexer".to_string())]
        );
        assert_eq!(
            valid_entries("=x1,2", "2 sketched the URL parser"),
            vec![(2, "sketched the URL parser".to_string())]
        );
        assert_eq!(
            valid_entries("=x1", "1 fixed 3 bugs"),
            vec![(1, "fixed 3 bugs".to_string())]
        );
        assert_eq!(
            valid_entries("=x", "1 wrote docs 1 opened the PR"),
            vec![
                (1, "wrote docs".to_string()),
                (1, "opened the PR".to_string())
            ]
        );
        assert_eq!(
            valid_entries("=x", "1 fixed \\3 bugs"),
            vec![(1, "fixed 3 bugs".to_string())]
        );
        assert_eq!(
            valid_entries("=x", "1 foo \\="),
            vec![(1, "foo =".to_string())]
        );
        assert_eq!(
            valid_entries("=x", "1 moved @@inbox"),
            vec![(1, "moved @@inbox".to_string())]
        );
    }

    #[test]
    fn rejects_bad_tails() {
        let not_worked = lex_tail("=x1", "2 foo").expect_err("not worked");
        assert!(
            not_worked.message.contains("isn't worked by `=x1`"),
            "{}",
            not_worked.message
        );
        let dropped = lex_tail("=x~2", "2 foo").expect_err("dropped");
        assert!(
            dropped.message.contains("is dropped by `~2`"),
            "{}",
            dropped.message
        );
        let empty = lex_tail("=x", "1 2 foo").expect_err("empty");
        assert!(
            empty.message.contains("type Work Log text after task 1"),
            "{}",
            empty.message
        );
        let block = lex_tail("=x", "1 see [[bob#^web-capture]]")
            .expect_err("block link");
        assert!(
            block.message.contains("can't contain the block link"),
            "{}",
            block.message
        );
        let fence = lex_tail("=x", "1 ```rust").expect_err("fence");
        assert!(
            fence.message.contains("can't start with a code fence"),
            "{}",
            fence.message
        );
    }

    #[test]
    fn reports_dangling() {
        match lex_tail("=x", "1").expect("dangling") {
            CloseLogOutcome::Dangling(dangling) => {
                assert_eq!(dangling.index, 1);
            }
            CloseLogOutcome::Valid(_) => panic!("expected dangling"),
        }
    }
}
