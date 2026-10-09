//! Shared `~<K>` drop-list lexer for whole-item Pomodoro starts.
//!
//! Both the execution parser (`bob capture`) and the editor parser
//! (`capture-parse`, completion, rewrite) lex a start drop list through
//! [`lex_start_drop`], so a mistyped list reports the same message and
//! byte range everywhere. Diagnostic text lives in [`markers`]; this module
//! only decides which diagnostic applies and where it points.

use super::markers::*;

/// A fully typed drop list: the `~<K>` numbers in ascending order.
/// `drop_range` covers `~<K>` including the `~`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartDropLex {
    pub(crate) drop: Vec<u32>,
    pub(crate) drop_range: (usize, usize),
}

/// An editing state: the token ends in a dangling separator. `separator` is
/// the `~` or `,` the user still has to follow with a task number, and
/// `drop` holds the numbers typed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartDropIncomplete {
    pub(crate) drop: Vec<u32>,
    pub(crate) drop_range: Option<(usize, usize)>,
    pub(crate) separator_range: (usize, usize),
    pub(crate) separator: char,
}

/// One lexical failure with its absolute byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartDropError {
    pub(crate) message: String,
    pub(crate) range: (usize, usize),
}

/// The result of lexing the text after a start's `~`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartDropOutcome {
    Valid(StartDropLex),
    Incomplete(StartDropIncomplete),
}

/// Lex the text after a start's first `~` (`after_tilde`), where
/// `base_offset` is the absolute byte offset of `after_tilde`'s first byte
/// and `token` is the display token (for example `=3#bugs~1,3`)
/// interpolated into diagnostics.
///
/// A trailing `~` or `,` is an [`StartDropOutcome::Incomplete`] editing
/// state, unless a lexical error elsewhere in the token wins. Anything else
/// malformed is a [`StartDropError`] with a precise range.
pub(crate) fn lex_start_drop(
    after_tilde: &str,
    base_offset: usize,
    token: &str,
) -> Result<StartDropOutcome, StartDropError> {
    let token_end = base_offset + after_tilde.len();
    let tilde_pos = base_offset.saturating_sub(1);
    // A second `~` points at the second tilde.
    if let Some(second) = after_tilde.find('~') {
        return Err(StartDropError {
            message: start_drop_one_tilde_error(),
            range: (base_offset + second, base_offset + second + 1),
        });
    }
    // `!` never belongs on a start: it completes links when closing.
    if let Some(bang) = after_tilde.find('!') {
        return Err(StartDropError {
            message: start_drop_bang_error(),
            range: (base_offset + bang, token_end),
        });
    }
    // A `#name` after the drop list belongs before it. Keep the typed `<X>`
    // in the suggestion (for example `=3~2#bugs` teaches `=3#bugs~2`),
    // spelling the typed `=`/`==` sigil.
    if let Some(hash) = after_tilde.find('#') {
        let numbers = &after_tilde[..hash];
        let name = &after_tilde[hash + 1..];
        let suffix = start_token_suffix(token);
        let sigil = if token.starts_with("==") { "==" } else { "=" };
        let suggestion = format!("{sigil}{suffix}#{name}~{numbers}");
        return Err(StartDropError {
            message: start_drop_hash_error(&suggestion, token),
            range: (base_offset + hash, token_end),
        });
    }
    // A dangling `~` with nothing after it (`=~`, `=3~`, `=#bugs~`).
    if after_tilde.is_empty() {
        return Ok(StartDropOutcome::Incomplete(StartDropIncomplete {
            drop: Vec::new(),
            drop_range: None,
            separator_range: (tilde_pos, base_offset),
            separator: '~',
        }));
    }
    // A trailing comma (`=~2,`) names no number after it yet.
    if let Some(body) = after_tilde.strip_suffix(',') {
        if body.is_empty() {
            return Err(StartDropError {
                message: close_selection_expected_number_error(),
                range: (token_end - 1, token_end),
            });
        }
        if body.ends_with(',') {
            return Err(StartDropError {
                message: close_selection_expected_number_error(),
                range: (token_end - 1, token_end),
            });
        }
        let drop = parse_drop_numbers(body, base_offset, token, token_end)?;
        validate_drop(&drop, token)?;
        let numbers: Vec<u32> =
            drop.iter().map(|(number, _)| *number).collect();
        let drop_range = (tilde_pos, tilde_pos + 1 + body.len());
        return Ok(StartDropOutcome::Incomplete(StartDropIncomplete {
            drop: sorted_numbers(&numbers),
            drop_range: Some(drop_range),
            separator_range: (token_end - 1, token_end),
            separator: ',',
        }));
    }
    let drop = parse_drop_numbers(after_tilde, base_offset, token, token_end)?;
    validate_drop(&drop, token)?;
    let numbers: Vec<u32> = drop.iter().map(|(number, _)| *number).collect();
    Ok(StartDropOutcome::Valid(StartDropLex {
        drop: sorted_numbers(&numbers),
        drop_range: (tilde_pos, token_end),
    }))
}

/// The typed `<X>` suffix of a whole-item start token (the `[0-9-]*` run
/// after `=`/`==`), so a `#name`-after-drop diagnostic can keep it in its
/// suggestion.
fn start_token_suffix(token: &str) -> &str {
    let rest = token
        .strip_prefix("==")
        .or_else(|| token.strip_prefix('='))
        .unwrap_or(token);
    let mut len = 0;
    while len < rest.len() && rest.as_bytes()[len].is_ascii_digit() {
        len += 1;
    }
    if rest.as_bytes().get(len) == Some(&b'-') {
        len += 1;
        while len < rest.len() && rest.as_bytes()[len].is_ascii_digit() {
            len += 1;
        }
    }
    &rest[..len]
}

type ParsedNumber = (u32, (usize, usize));

/// Parse one comma-separated drop-number list. An empty element is an
/// "expected a task number" error on its comma; any other bad byte fails
/// from that byte through the token end.
fn parse_drop_numbers(
    part: &str,
    part_base: usize,
    token: &str,
    token_end: usize,
) -> Result<Vec<ParsedNumber>, StartDropError> {
    if part.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = part.as_bytes();
    let mut numbers = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let digits_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if digits_start == index {
            if bytes[index] == b',' {
                return Err(StartDropError {
                    message: close_selection_expected_number_error(),
                    range: (part_base + index, part_base + index + 1),
                });
            }
            return Err(StartDropError {
                message: start_drop_bad_list_error(token),
                range: (part_base + index, token_end),
            });
        }
        let digits = &part[digits_start..index];
        let range = (part_base + digits_start, part_base + index);
        let number = digits.parse::<u32>().map_err(|_| StartDropError {
            message: close_selection_too_large_error(digits),
            range,
        })?;
        numbers.push((number, range));
        if index == bytes.len() {
            break;
        }
        if bytes[index] == b',' {
            index += 1;
            if index == bytes.len() {
                return Err(StartDropError {
                    message: close_selection_expected_number_error(),
                    range: (part_base + index - 1, part_base + index),
                });
            }
            continue;
        }
        return Err(StartDropError {
            message: start_drop_bad_list_error(token),
            range: (part_base + index, token_end),
        });
    }
    Ok(numbers)
}

/// Validate zeros and duplicates, then sort ascending.
fn validate_drop(
    parsed: &[ParsedNumber],
    token: &str,
) -> Result<(), StartDropError> {
    if let Some((_, range)) = parsed.iter().find(|(number, _)| *number == 0) {
        return Err(StartDropError {
            message: close_selection_starts_at_one_error(),
            range: *range,
        });
    }
    for (position, (number, range)) in parsed.iter().enumerate() {
        if parsed[..position].iter().any(|(seen, _)| seen == number) {
            return Err(StartDropError {
                message: close_selection_duplicate_error(*number, token),
                range: *range,
            });
        }
    }
    Ok(())
}

fn sorted_numbers(numbers: &[u32]) -> Vec<u32> {
    let mut sorted = numbers.to_vec();
    sorted.sort_unstable();
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(token: &str) -> Result<StartDropOutcome, StartDropError> {
        let tilde = token.find('~').expect("has tilde");
        let after = &token[tilde + 1..];
        lex_start_drop(after, tilde + 1, token)
    }

    fn valid(token: &str) -> StartDropLex {
        match lex(token) {
            Ok(StartDropOutcome::Valid(lex)) => lex,
            other => panic!("{token}: expected valid, got {other:?}"),
        }
    }

    fn incomplete(token: &str) -> StartDropIncomplete {
        match lex(token) {
            Ok(StartDropOutcome::Incomplete(incomplete)) => incomplete,
            other => panic!("{token}: expected incomplete, got {other:?}"),
        }
    }

    fn error(token: &str) -> StartDropError {
        match lex(token) {
            Err(error) => error,
            other => panic!("{token}: expected error, got {other:?}"),
        }
    }

    #[test]
    fn lex_reports_valid_drop_lists() {
        let one = valid("=~2");
        assert_eq!(one.drop, vec![2]);
        assert_eq!(one.drop_range, (1, 3));

        let sorted = valid("=3~2,4");
        assert_eq!(sorted.drop, vec![2, 4]);

        let unsorted = valid("=3#bugs~3,1");
        assert_eq!(unsorted.drop, vec![1, 3]);
        assert_eq!(unsorted.drop_range, (7, 11));
    }

    #[test]
    fn lex_reports_dangling_separators_as_incomplete() {
        let bare = incomplete("=~");
        assert_eq!(bare.drop, Vec::<u32>::new());
        assert_eq!(bare.drop_range, None);
        assert_eq!(bare.separator_range, (1, 2));
        assert_eq!(bare.separator, '~');

        let trailing = incomplete("=~2,");
        assert_eq!(trailing.drop, vec![2]);
        assert_eq!(trailing.drop_range, Some((1, 3)));
        assert_eq!(trailing.separator_range, (3, 4));
        assert_eq!(trailing.separator, ',');
    }

    #[test]
    fn lex_reports_precise_diagnostics() {
        let zero = error("=~0");
        assert_eq!(zero.message, "task numbers start at 1");

        let duplicate = error("=~2,2");
        assert_eq!(duplicate.message, "task 2 is listed twice in `=~2,2`");

        let leading = error("=~,2");
        assert_eq!(leading.message, "expected a task number before `,`");

        let doubled = error("=~2,,3");
        assert_eq!(doubled.message, "expected a task number before `,`");

        let second = error("=~2~3");
        assert_eq!(second.message, "use one `~` list: `=~2,3`");
        assert_eq!(second.range, (3, 4));

        let bang = error("=~2!3");
        assert_eq!(
            bang.message,
            "a start can only drop Task Links; `!` completes them when you close (`=x!3`)"
        );

        let hash = error("=~2#bugs");
        assert_eq!(
            hash.message,
            "write the drop list after the name: `=#bugs~2` instead of `=~2#bugs`"
        );

        let bad = error("=~2a");
        assert!(bad.message.starts_with("`=~2a` is not a drop list:"));

        let overflow = error("=~99999999999");
        assert_eq!(overflow.message, "task number 99999999999 is too large");
    }
}
