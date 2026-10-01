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
//! authored-bullet line rules. A first-level bullet is an entry: its first
//! whitespace-separated token is the task number and everything after it is
//! the entry text. A two-space nested bullet is a detail of the nearest
//! preceding entry and its text is entirely literal. Loggability is purely
//! lexical: with `<N>` typed (including `=x0`) only the numbers in `<N>` or
//! `!<M>` start entries, otherwise every number `>= 1` starts one except
//! those in `~<K>`. Every backslash is literal.

use super::draft::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use crate::native::capture_pomodoro_close::wikilink_tokens;

/// One lexed Work Log entry: the numbered Task Link plus its literal text,
/// its nested detail lines, and the absolute byte ranges of the index token,
/// the entry text (first text token start through last text token end), and
/// each detail (first token start through last token end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseLogEntryLex {
    pub(crate) index: u32,
    pub(crate) text: String,
    pub(crate) details: Vec<String>,
    pub(crate) index_range: (usize, usize),
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

/// The smallest loggable task number: the smallest of `<N>` union `*<P>`
/// union `!<M>` when selection mode is active and the union is non-empty,
/// otherwise `1`. Used to build the missing-number example from the bullet's
/// own text.
fn smallest_loggable(
    in_progress: Option<&[u32]>,
    park: &[u32],
    complete: &[u32],
) -> u32 {
    if in_progress.is_some() || !park.is_empty() {
        let mut smallest: Option<u32> = None;
        for number in in_progress
            .iter()
            .flat_map(|list| list.iter())
            .chain(park.iter())
            .chain(complete.iter())
        {
            smallest = Some(match smallest {
                Some(current) => current.min(*number),
                None => *number,
            });
        }
        smallest.unwrap_or(1)
    } else {
        1
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

/// Lex a close's Work Log bullets: the close item's child `ItemLine`s.
/// `close_token` is the display token (`=x`, `=x2`, `=x1*2!3`) interpolated
/// into not-worked diagnostics; `in_progress`/`park`/`complete`/`drop` are
/// the lexed selection lists.
///
/// Placeholder rows are skipped. Invalid and orphaned lines report the
/// existing authored-bullet messages. Bullets are checked top to bottom and
/// the first error wins; any error outranks a dangling bullet.
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
        // First-level bullet: the first token must be a task number.
        seen_first_level = true;
        let first = &tokens[0];
        if first.text.is_empty()
            || !first.text.bytes().all(|byte| byte.is_ascii_digit())
        {
            let normalized = normalize_task_text(authored.body);
            let smallest = smallest_loggable(in_progress, park, complete);
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
            index,
            text,
            details: Vec::new(),
            index_range: (first.start, first.end),
            text_range,
            detail_ranges: Vec::new(),
        });
        last_entry = Some(Some(entries.len() - 1));
    }

    Ok(CloseLogLexed { entries, dangling })
}

/// Build the parent-line bullet hint for extra text after the close token.
/// `tail_tokens` are the whitespace-separated tokens after the close token.
/// Returns the echoed `- <n> <text>` bullet when the tail starts with a
/// positive number (after an optional stray `-`/`*`/`+` token), or `None`
/// for the generic hint.
pub(crate) fn close_tail_bullet_hint(
    tail_tokens: &[Token<'_>],
) -> Option<String> {
    let words: Vec<&str> = tail_tokens.iter().map(|token| token.text).collect();
    let words: &[&str] = match words.first() {
        Some(first) if matches!(*first, "-" | "*" | "+") => words.get(1..)?,
        Some(_) => &words,
        None => return None,
    };
    let Some(head) = words.first() else {
        return None;
    };
    if head.is_empty()
        || !head.bytes().all(|byte| byte.is_ascii_digit())
        || *head == "0"
    {
        return None;
    }
    Some(format!("- {}", words.join(" ")))
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
            details: entry.details.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::close_selection::*;
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
    ) -> Vec<(u32, String, Vec<String>)> {
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
            vec![(1, "wired the lexer".to_string(), Vec::new())]
        );
        assert_eq!(
            valid_entries("=x1,2", "=x1,2\n- 1 wired the lexer\n  - chose a hand-rolled lexer\n- 2 sketched the URL parser"),
            vec![
                (
                    1,
                    "wired the lexer".to_string(),
                    vec!["chose a hand-rolled lexer".to_string()]
                ),
                (2, "sketched the URL parser".to_string(), Vec::new()),
            ]
        );
        // Only the first token is an index: later numbers stay text.
        assert_eq!(
            valid_entries("=x1", "=x1\n- 1 fixed 3 bugs"),
            vec![(1, "fixed 3 bugs".to_string(), Vec::new())]
        );
        // Markers stay literal in bullets.
        assert_eq!(
            valid_entries("=x", "=x\n- 1 moved @@inbox s:3"),
            vec![(1, "moved @@inbox s:3".to_string(), Vec::new())]
        );
        // Backslashes stay literal: no escape.
        assert_eq!(
            valid_entries("=x", "=x\n- 1 fixed \\3 bugs"),
            vec![(1, "fixed \\3 bugs".to_string(), Vec::new())]
        );
        // Placeholders are skipped.
        assert_eq!(
            valid_entries("=x", "=x\n- 1 wired the lexer\n- "),
            vec![(1, "wired the lexer".to_string(), Vec::new())]
        );
    }

    #[test]
    fn rejects_bad_bullets() {
        let missing = lex_bullets("=x", "=x\n- wired the lexer")
            .expect_err("missing number");
        assert!(
            missing.message.contains("start each Work Log bullet"),
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

    #[test]
    fn bullet_hint_echoes_numbers() {
        let tokens = tokenize_with_spans("2 foo bar baz");
        assert_eq!(
            close_tail_bullet_hint(&tokens),
            Some("- 2 foo bar baz".to_string())
        );
        let stray = tokenize_with_spans("- 2 foo");
        assert_eq!(close_tail_bullet_hint(&stray), Some("- 2 foo".to_string()));
        let prose = tokenize_with_spans("more");
        assert_eq!(close_tail_bullet_hint(&prose), None);
        let zero = tokenize_with_spans("0 foo");
        assert_eq!(close_tail_bullet_hint(&zero), None);
    }
}
