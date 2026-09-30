//! Shared `=x[<N>][!<M>][~<K>]` selection lexer for Pomodoro closes.
//!
//! Both the execution parser (`bob capture`) and the editor parser
//! (`capture-parse`, completion, rewrite) lex a close selection through
//! [`lex_close_selection`], so a mistyped list reports the same message and
//! byte range everywhere. Diagnostic text lives in [`markers`]; this module
//! only decides which diagnostic applies and where it points.

use super::markers::*;
use super::model::*;

/// A fully typed selection: `<N>` (or `None` when omitted, so unlisted links
/// keep their ledger outcome) plus the `!<M>` and `~<K>` lists. Ranges are
/// absolute byte offsets: `in_progress_range` covers `<N>` including its
/// commas, `complete_range` covers `!<M>` including the `!`, and
/// `drop_range` covers `~<K>` including the `~`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseSelectionLex {
    pub(crate) in_progress: Option<Vec<u32>>,
    pub(crate) complete: Vec<u32>,
    pub(crate) drop: Vec<u32>,
    pub(crate) in_progress_range: Option<(usize, usize)>,
    pub(crate) complete_range: Option<(usize, usize)>,
    pub(crate) drop_range: Option<(usize, usize)>,
}

/// An editing state: the token ends in a dangling separator. `separator` is
/// the `,`, `!`, or `~` the user still has to follow with a task number,
/// and the remaining fields describe the lists typed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseSelectionIncomplete {
    pub(crate) in_progress: Option<Vec<u32>>,
    pub(crate) complete: Vec<u32>,
    pub(crate) drop: Vec<u32>,
    pub(crate) in_progress_range: Option<(usize, usize)>,
    pub(crate) complete_range: Option<(usize, usize)>,
    pub(crate) drop_range: Option<(usize, usize)>,
    pub(crate) separator_range: (usize, usize),
    pub(crate) separator: char,
}

/// One lexical failure with its absolute byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseSelectionError {
    pub(crate) message: String,
    pub(crate) range: (usize, usize),
}

/// The result of lexing the text after a close's `x`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseSelectionOutcome {
    Valid(CloseSelectionLex),
    Incomplete(CloseSelectionIncomplete),
}

/// Return the text after the `x` when a whole-item token is
/// selection-shaped: it starts with `=x`/`=X` and the character right after
/// the `x` is an ASCII digit, `,`, `!`, or `~`. `None` for plain `=x`, every
/// other `=` shape (`=xx`, `=xa`, `=x.`), and non-close tokens.
pub(crate) fn whole_item_close_after_x(token: &str) -> Option<&str> {
    let after_eq = token.strip_prefix('=')?;
    link_close_after_x(after_eq)
}

/// Return the text after the `x` when a link suffix (the text after `=`) is
/// selection-shaped: it starts with `x`/`X` and the character right after
/// the `x` is an ASCII digit, `,`, `!`, or `~`. `None` for a plain `x`
/// suffix and every `=<X>` start shape.
pub(crate) fn link_close_after_x(suffix: &str) -> Option<&str> {
    let after_x = suffix
        .strip_prefix('x')
        .or_else(|| suffix.strip_prefix('X'))?;
    if after_x.as_bytes().first().is_some_and(|byte| {
        byte.is_ascii_digit() || *byte == b',' || *byte == b'!' || *byte == b'~'
    }) {
        Some(after_x)
    } else {
        None
    }
}

/// Lex the text after a close's `x` (`after_x`), where `base_offset` is the
/// absolute byte offset of `after_x`'s first byte and `token` is the display
/// token (`=x...`) interpolated into diagnostics.
///
/// An empty `after_x` is a plain close. A trailing `,`, `!`, or `~` is an
/// [`CloseSelectionOutcome::Incomplete`] editing state, unless a lexical
/// error elsewhere in the token wins. Anything else malformed is a
/// [`CloseSelectionError`] with a precise range.
pub(crate) fn lex_close_selection(
    after_x: &str,
    base_offset: usize,
    token: &str,
) -> Result<CloseSelectionOutcome, CloseSelectionError> {
    if after_x.is_empty() {
        return Ok(CloseSelectionOutcome::Valid(CloseSelectionLex {
            in_progress: None,
            complete: Vec::new(),
            drop: Vec::new(),
            in_progress_range: None,
            complete_range: None,
            drop_range: None,
        }));
    }
    let token_end = base_offset + after_x.len();
    let bang_count = after_x.bytes().filter(|byte| *byte == b'!').count();
    if bang_count > 1 {
        let first = after_x.find('!').expect("has bang");
        let second = after_x[first + 1..]
            .find('!')
            .map(|offset| first + 1 + offset)
            .expect("second bang");
        return Err(CloseSelectionError {
            message: close_selection_one_bang_error(),
            range: (base_offset + second, base_offset + second + 1),
        });
    }
    let tilde_count = after_x.bytes().filter(|byte| *byte == b'~').count();
    if tilde_count > 1 {
        let first = after_x.find('~').expect("has tilde");
        let second = after_x[first + 1..]
            .find('~')
            .map(|offset| first + 1 + offset)
            .expect("second tilde");
        return Err(CloseSelectionError {
            message: close_selection_one_tilde_error(),
            range: (base_offset + second, base_offset + second + 1),
        });
    }
    if let Some(body) = after_x.strip_suffix('!') {
        let parsed = parse_selection_body(body, base_offset, token, token_end)?;
        return Ok(CloseSelectionOutcome::Incomplete(
            CloseSelectionIncomplete {
                separator_range: (token_end - 1, token_end),
                separator: '!',
                in_progress: parsed.in_progress,
                complete: Vec::new(),
                drop: parsed.drop,
                in_progress_range: parsed.in_progress_range,
                complete_range: None,
                drop_range: parsed.drop_range,
            },
        ));
    }
    if let Some(body) = after_x.strip_suffix('~') {
        let parsed = parse_selection_body(body, base_offset, token, token_end)?;
        return Ok(CloseSelectionOutcome::Incomplete(
            CloseSelectionIncomplete {
                separator_range: (token_end - 1, token_end),
                separator: '~',
                in_progress: parsed.in_progress,
                complete: parsed.complete,
                drop: Vec::new(),
                in_progress_range: parsed.in_progress_range,
                complete_range: parsed.complete_range,
                drop_range: None,
            },
        ));
    }
    if let Some(head) = after_x.strip_suffix(',') {
        let separator_range = (token_end - 1, token_end);
        if !head.contains('!') && !head.contains('~') && head.is_empty() {
            return Err(CloseSelectionError {
                message: close_selection_expected_number_error(),
                range: separator_range,
            });
        }
        // A dangling list separator (`=x1!,`, `=x1~2~,`) names no number
        // after it yet: the trailing comma is the precise range.
        if let Some(last) = head.rfind(['!', '~'])
            && head[last + 1..].is_empty()
        {
            return Err(CloseSelectionError {
                message: close_selection_expected_number_error(),
                range: separator_range,
            });
        }
        // `=x0,` can never become valid: `0` must stand alone, so the
        // lexical `0`-alone error wins over the incomplete state.
        if head == "0" {
            return Err(CloseSelectionError {
                message: close_selection_zero_alone_error(),
                range: (base_offset, base_offset + 1),
            });
        }
        // A double separator (`=x1,,`, `=x1!2,,`) points at the dangling
        // (second) comma, matching `=x1,,2`.
        if head.ends_with(',') {
            return Err(CloseSelectionError {
                message: close_selection_expected_number_error(),
                range: separator_range,
            });
        }
        let parsed = parse_selection_body(head, base_offset, token, token_end)?;
        return Ok(CloseSelectionOutcome::Incomplete(
            CloseSelectionIncomplete {
                separator_range,
                separator: ',',
                in_progress: parsed.in_progress,
                complete: parsed.complete,
                drop: parsed.drop,
                in_progress_range: parsed.in_progress_range,
                complete_range: parsed.complete_range,
                drop_range: parsed.drop_range,
            },
        ));
    }
    let parsed = parse_selection_body(after_x, base_offset, token, token_end)?;
    Ok(CloseSelectionOutcome::Valid(CloseSelectionLex {
        in_progress: parsed.in_progress,
        complete: parsed.complete,
        drop: parsed.drop,
        in_progress_range: parsed.in_progress_range,
        complete_range: parsed.complete_range,
        drop_range: parsed.drop_range,
    }))
}

/// The parsed lists plus their ranges, before the valid/incomplete split.
struct SelectionBody {
    in_progress: Option<Vec<u32>>,
    complete: Vec<u32>,
    drop: Vec<u32>,
    in_progress_range: Option<(usize, usize)>,
    complete_range: Option<(usize, usize)>,
    drop_range: Option<(usize, usize)>,
}

/// One labeled trailing list: the `!` complete list or the `~` drop list,
/// in the order typed.
struct TrailingList<'a> {
    separator: char,
    text: &'a str,
    base: usize,
}

/// Parse a complete selection body (no trailing separator): split on the
/// single `!` and the single `~` (either order), parse every list, then
/// validate zeros, duplicates, and overlaps. An empty `<N>` with a
/// `!` or `~` is an omitted list; an empty `<N>` without one is only
/// reachable for an empty body, which callers handle.
fn parse_selection_body(
    body: &str,
    base_offset: usize,
    token: &str,
    token_end: usize,
) -> Result<SelectionBody, CloseSelectionError> {
    let bang = body.find('!');
    let tilde = body.find('~');
    let (n_text, trailing) = match (bang, tilde) {
        (None, None) => (body, Vec::new()),
        (Some(bang), None) => (
            &body[..bang],
            vec![TrailingList {
                separator: '!',
                text: &body[bang + 1..],
                base: base_offset + bang + 1,
            }],
        ),
        (None, Some(tilde)) => (
            &body[..tilde],
            vec![TrailingList {
                separator: '~',
                text: &body[tilde + 1..],
                base: base_offset + tilde + 1,
            }],
        ),
        (Some(bang), Some(tilde)) => {
            let (first, second) = if bang < tilde {
                (bang, tilde)
            } else {
                (tilde, bang)
            };
            let first_sep = body.as_bytes()[first] as char;
            let second_sep = body.as_bytes()[second] as char;
            (
                &body[..first],
                vec![
                    TrailingList {
                        separator: first_sep,
                        text: &body[first + 1..second],
                        base: base_offset + first + 1,
                    },
                    TrailingList {
                        separator: second_sep,
                        text: &body[second + 1..],
                        base: base_offset + second + 1,
                    },
                ],
            )
        }
    };
    let has_trailing = !trailing.is_empty();
    // `=x1,!2`: the `<N>` part ends in a single comma after a number, so the
    // empty element sits after the `,` (before the separator). Report the
    // new "after" message on that comma instead of the generic "before".
    if has_trailing
        && n_text.ends_with(',')
        && !n_text.ends_with(",,")
        && n_text.len() >= 2
        && n_text.as_bytes()[n_text.len() - 2].is_ascii_digit()
    {
        let comma = base_offset + n_text.len() - 1;
        return Err(CloseSelectionError {
            message: close_selection_expected_number_after_error(),
            range: (comma, comma + 1),
        });
    }
    let n_parsed = parse_number_list(n_text, base_offset, token, token_end)?;
    let mut m_parsed: Vec<ParsedNumber> = Vec::new();
    let mut complete_range = None;
    let mut k_parsed: Vec<ParsedNumber> = Vec::new();
    let mut drop_range = None;
    for list in &trailing {
        let parsed = parse_number_list(list.text, list.base, token, token_end)?;
        // The range covers the separator plus the list text.
        let range = (list.base - 1, list.base + list.text.len());
        match list.separator {
            '!' => {
                m_parsed = parsed;
                complete_range = Some(range);
            }
            _ => {
                k_parsed = parsed;
                drop_range = Some(range);
            }
        }
    }
    validate_selection(
        n_text,
        has_trailing,
        n_parsed,
        m_parsed,
        k_parsed,
        base_offset,
        token,
        complete_range,
        drop_range,
    )
}

/// One parsed task number plus its absolute byte range.
type ParsedNumber = (u32, (usize, usize));

/// Parse one comma-separated number list (no `!`): ASCII digits whose value
/// fits in `u32`. An empty element is an "expected a task number" error on
/// its comma; any other bad byte fails from that byte through the token end.
fn parse_number_list(
    part: &str,
    part_base: usize,
    token: &str,
    token_end: usize,
) -> Result<Vec<ParsedNumber>, CloseSelectionError> {
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
                return Err(CloseSelectionError {
                    message: close_selection_expected_number_error(),
                    range: (part_base + index, part_base + index + 1),
                });
            }
            return Err(CloseSelectionError {
                message: close_selection_bad_list_error(token),
                range: (part_base + index, token_end),
            });
        }
        let digits = &part[digits_start..index];
        let range = (part_base + digits_start, part_base + index);
        let number =
            digits.parse::<u32>().map_err(|_| CloseSelectionError {
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
                return Err(CloseSelectionError {
                    message: close_selection_expected_number_error(),
                    range: (part_base + index - 1, part_base + index),
                });
            }
            continue;
        }
        return Err(CloseSelectionError {
            message: close_selection_bad_list_error(token),
            range: (part_base + index, token_end),
        });
    }
    Ok(numbers)
}

/// Validate zeros, duplicates, and overlaps across all three lists, then
/// sort ascending. `n_text`/`has_trailing` decide whether an empty `<N>` is
/// omitted (`None`) or explicit (`Some`, only for a bare `0`).
#[allow(clippy::too_many_arguments)]
fn validate_selection(
    n_text: &str,
    has_trailing: bool,
    n_parsed: Vec<ParsedNumber>,
    m_parsed: Vec<ParsedNumber>,
    k_parsed: Vec<ParsedNumber>,
    base_offset: usize,
    token: &str,
    complete_range: Option<(usize, usize)>,
    drop_range: Option<(usize, usize)>,
) -> Result<SelectionBody, CloseSelectionError> {
    // Only a literal `0` means "none": a zero-valued `<n>` with extra digits
    // (`=x00`) gets the `0`-alone message on that number.
    if let Some((_, range)) = n_parsed.iter().find(|(number, range)| {
        *number == 0 && range.1.saturating_sub(range.0) > 1
    }) {
        return Err(CloseSelectionError {
            message: close_selection_zero_alone_error(),
            range: *range,
        });
    }
    if let Some((_, range)) = n_parsed.iter().find(|(number, _)| *number == 0)
        && n_parsed.len() > 1
    {
        return Err(CloseSelectionError {
            message: close_selection_zero_alone_error(),
            range: *range,
        });
    }
    if let Some((_, range)) = m_parsed
        .iter()
        .chain(k_parsed.iter())
        .find(|(number, _)| *number == 0)
    {
        return Err(CloseSelectionError {
            message: close_selection_starts_at_one_error(),
            range: *range,
        });
    }
    for (position, (number, range)) in n_parsed.iter().enumerate() {
        if n_parsed[..position].iter().any(|(seen, _)| seen == number) {
            return Err(CloseSelectionError {
                message: close_selection_duplicate_error(*number, token),
                range: *range,
            });
        }
    }
    for (position, (number, range)) in m_parsed.iter().enumerate() {
        if m_parsed[..position].iter().any(|(seen, _)| seen == number) {
            return Err(CloseSelectionError {
                message: close_selection_duplicate_error(*number, token),
                range: *range,
            });
        }
    }
    for (position, (number, range)) in k_parsed.iter().enumerate() {
        if k_parsed[..position].iter().any(|(seen, _)| seen == number) {
            return Err(CloseSelectionError {
                message: close_selection_duplicate_error(*number, token),
                range: *range,
            });
        }
    }
    if let Some((overlapped, range)) = m_parsed
        .iter()
        .find(|(number, _)| n_parsed.iter().any(|(seen, _)| seen == number))
    {
        return Err(CloseSelectionError {
            message: close_selection_overlap_error(*overlapped, token),
            range: *range,
        });
    }
    if let Some((overlapped, range)) = k_parsed
        .iter()
        .find(|(number, _)| n_parsed.iter().any(|(seen, _)| seen == number))
    {
        return Err(CloseSelectionError {
            message: close_selection_overlap_in_progress_drop_error(
                *overlapped,
                token,
            ),
            range: *range,
        });
    }
    if let Some((overlapped, range)) = k_parsed
        .iter()
        .find(|(number, _)| m_parsed.iter().any(|(seen, _)| seen == number))
    {
        return Err(CloseSelectionError {
            message: close_selection_overlap_complete_drop_error(
                *overlapped,
                token,
            ),
            range: *range,
        });
    }
    let in_progress =
        if (n_text.is_empty() && has_trailing) || n_parsed.is_empty() {
            None
        } else {
            let mut numbers: Vec<u32> =
                n_parsed.iter().map(|(number, _)| *number).collect();
            numbers.sort_unstable();
            Some(numbers)
        };
    // A lone `0` means none: keep the typed range but report no numbers.
    let in_progress = match &in_progress {
        Some(numbers) if numbers == &[0] => Some(Vec::new()),
        other => other.clone(),
    };
    let in_progress_range = if n_text.is_empty() {
        None
    } else {
        Some((base_offset, base_offset + n_text.len()))
    };
    let mut complete: Vec<u32> =
        m_parsed.iter().map(|(number, _)| *number).collect();
    complete.sort_unstable();
    let mut drop: Vec<u32> =
        k_parsed.iter().map(|(number, _)| *number).collect();
    drop.sort_unstable();
    Ok(SelectionBody {
        in_progress,
        complete,
        drop,
        in_progress_range,
        complete_range,
        drop_range,
    })
}

/// Build a [`PomodoroCloseSpec`] from a valid lex result.
pub(crate) fn close_spec_from_lex(
    raw: String,
    lex: &CloseSelectionLex,
) -> PomodoroCloseSpec {
    PomodoroCloseSpec {
        raw,
        in_progress: lex.in_progress.clone(),
        complete: lex.complete.clone(),
        drop: lex.drop.clone(),
        log: Vec::new(),
    }
}

/// Build a partial [`PomodoroCloseSpec`] from an incomplete lex result: the
/// lists typed so far (`=x1,` gives `in_progress: [1]`; `=x!` gives
/// `in_progress: None, complete: []`).
pub(crate) fn close_spec_from_incomplete(
    raw: String,
    incomplete: &CloseSelectionIncomplete,
) -> PomodoroCloseSpec {
    PomodoroCloseSpec {
        raw,
        in_progress: incomplete.in_progress.clone(),
        complete: incomplete.complete.clone(),
        drop: incomplete.drop.clone(),
        log: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(token: &str) -> Result<CloseSelectionOutcome, CloseSelectionError> {
        let after_x = whole_item_close_after_x(token).expect("shaped");
        let base = token.len() - after_x.len();
        lex_close_selection(after_x, base, token)
    }

    fn valid(token: &str) -> CloseSelectionLex {
        match lex(token) {
            Ok(CloseSelectionOutcome::Valid(lex)) => lex,
            other => panic!("{token}: expected valid, got {other:?}"),
        }
    }

    fn incomplete(token: &str) -> CloseSelectionIncomplete {
        match lex(token) {
            Ok(CloseSelectionOutcome::Incomplete(incomplete)) => incomplete,
            other => panic!("{token}: expected incomplete, got {other:?}"),
        }
    }

    fn error(token: &str) -> CloseSelectionError {
        match lex(token) {
            Err(error) => error,
            other => panic!("{token}: expected error, got {other:?}"),
        }
    }

    #[test]
    fn lex_reports_valid_selections() {
        let plain = valid("=x1");
        assert_eq!(plain.in_progress, Some(vec![1]));
        assert_eq!(plain.complete, Vec::<u32>::new());
        assert_eq!(plain.in_progress_range, Some((2, 3)));
        assert_eq!(plain.complete_range, None);

        let both = valid("=x1,3!2");
        assert_eq!(both.in_progress, Some(vec![1, 3]));
        assert_eq!(both.complete, vec![2]);
        assert_eq!(both.in_progress_range, Some((2, 5)));
        assert_eq!(both.complete_range, Some((5, 7)));

        // List order does not matter; JSON reports ascending.
        let sorted = valid("=x3,1");
        assert_eq!(sorted.in_progress, Some(vec![1, 3]));

        let upper = valid("=X1!2");
        assert_eq!(upper.in_progress, Some(vec![1]));
        assert_eq!(upper.complete, vec![2]);

        let complete_only = valid("=x!2");
        assert_eq!(complete_only.in_progress, None);
        assert_eq!(complete_only.complete, vec![2]);
        assert_eq!(complete_only.in_progress_range, None);
        assert_eq!(complete_only.complete_range, Some((2, 4)));

        let none = valid("=x0");
        assert_eq!(none.in_progress, Some(Vec::new()));
        assert_eq!(none.complete, Vec::<u32>::new());
        assert_eq!(none.in_progress_range, Some((2, 3)));

        let none_complete = valid("=x0!2");
        assert_eq!(none_complete.in_progress, Some(Vec::new()));
        assert_eq!(none_complete.complete, vec![2]);
    }

    #[test]
    fn lex_reports_precise_diagnostics() {
        let duplicate = error("=x1,1");
        assert_eq!(duplicate.message, "task 1 is listed twice in `=x1,1`");
        assert_eq!(duplicate.range, (4, 5));

        let overlap = error("=x1!1");
        assert_eq!(
            overlap.message,
            "task 1 cannot both stay in progress and complete in `=x1!1`"
        );
        assert_eq!(overlap.range, (4, 5));

        let overlap_later = error("=x1,2!2");
        assert_eq!(
            overlap_later.message,
            "task 2 cannot both stay in progress and complete in `=x1,2!2`"
        );
        assert_eq!(overlap_later.range, (6, 7));

        let zero = error("=x0,2");
        assert_eq!(
            zero.message,
            "`0` means no task stays in progress; use it alone, as `=x0`, `=x0!2`, or `=x0~2`"
        );
        assert_eq!(zero.range, (2, 3));

        let zero_later = error("=x2,0");
        assert_eq!(zero_later.range, (4, 5));

        let starts_at_one = error("=x!0");
        assert_eq!(starts_at_one.message, "task numbers start at 1");
        assert_eq!(starts_at_one.range, (3, 4));

        let leading_comma = error("=x,1");
        assert_eq!(leading_comma.message, "expected a task number before `,`");
        assert_eq!(leading_comma.range, (2, 3));

        let doubled_comma = error("=x1,,2");
        assert_eq!(doubled_comma.range, (4, 5));

        let doubled_bang = error("=x1!2!3");
        assert_eq!(doubled_bang.message, "use one `!` list: `=x1!2,3`");
        assert_eq!(doubled_bang.range, (5, 6));

        let bad = error("=x1a");
        assert!(
            bad.message.starts_with("`=x1a` is not a task list:"),
            "{}",
            bad.message
        );
        assert_eq!(bad.range, (3, 4));

        let bad_mid = error("=x1;2");
        assert_eq!(bad_mid.range, (3, 5));

        let overflow = error("=x99999999999");
        assert_eq!(overflow.message, "task number 99999999999 is too large");
        assert_eq!(overflow.range, (2, 13));

        let double_trailing = error("=x1,,");
        assert_eq!(
            double_trailing.message,
            "expected a task number before `,`"
        );
        assert_eq!(double_trailing.range, (4, 5));

        let double_trailing_complete = error("=x1!2,,");
        assert_eq!(double_trailing_complete.range, (6, 7));

        let after_comma = error("=x1,!2");
        assert_eq!(after_comma.message, "expected a task number after `,`");
        assert_eq!(after_comma.range, (3, 4));

        let zero_trailing = error("=x0,");
        assert_eq!(
            zero_trailing.message,
            "`0` means no task stays in progress; use it alone, as `=x0`, `=x0!2`, or `=x0~2`"
        );
        assert_eq!(zero_trailing.range, (2, 3));

        let zero_padded = error("=x00");
        assert_eq!(
            zero_padded.message,
            "`0` means no task stays in progress; use it alone, as `=x0`, `=x0!2`, or `=x0~2`"
        );
        assert_eq!(zero_padded.range, (2, 4));
    }

    #[test]
    fn lex_reports_drop_selections() {
        let drop_only = valid("=x~2");
        assert_eq!(drop_only.in_progress, None);
        assert_eq!(drop_only.complete, Vec::<u32>::new());
        assert_eq!(drop_only.drop, vec![2]);
        assert_eq!(drop_only.in_progress_range, None);
        assert_eq!(drop_only.complete_range, None);
        assert_eq!(drop_only.drop_range, Some((2, 4)));

        let in_progress_drop = valid("=x1~2");
        assert_eq!(in_progress_drop.in_progress, Some(vec![1]));
        assert_eq!(in_progress_drop.drop, vec![2]);
        assert_eq!(in_progress_drop.in_progress_range, Some((2, 3)));
        assert_eq!(in_progress_drop.drop_range, Some((3, 5)));

        // `!` and `~` compose in either order with the same lists.
        let complete_then_drop = valid("=x1!2~3");
        assert_eq!(complete_then_drop.in_progress, Some(vec![1]));
        assert_eq!(complete_then_drop.complete, vec![2]);
        assert_eq!(complete_then_drop.drop, vec![3]);
        assert_eq!(complete_then_drop.in_progress_range, Some((2, 3)));
        assert_eq!(complete_then_drop.complete_range, Some((3, 5)));
        assert_eq!(complete_then_drop.drop_range, Some((5, 7)));

        let drop_then_complete = valid("=x1~3!2");
        assert_eq!(drop_then_complete.in_progress, Some(vec![1]));
        assert_eq!(drop_then_complete.complete, vec![2]);
        assert_eq!(drop_then_complete.drop, vec![3]);
        assert_eq!(drop_then_complete.in_progress_range, Some((2, 3)));
        assert_eq!(drop_then_complete.drop_range, Some((3, 5)));
        assert_eq!(drop_then_complete.complete_range, Some((5, 7)));

        let omitted_both = valid("=x!2~3");
        assert_eq!(omitted_both.in_progress, None);
        assert_eq!(omitted_both.complete, vec![2]);
        assert_eq!(omitted_both.drop, vec![3]);

        let reversed_omitted = valid("=x~3!2");
        assert_eq!(reversed_omitted.in_progress, None);
        assert_eq!(reversed_omitted.complete, vec![2]);
        assert_eq!(reversed_omitted.drop, vec![3]);

        // List order does not matter; JSON reports ascending.
        let sorted = valid("=x~3,1");
        assert_eq!(sorted.drop, vec![1, 3]);

        let none_drop = valid("=x0~2");
        assert_eq!(none_drop.in_progress, Some(Vec::new()));
        assert_eq!(none_drop.drop, vec![2]);

        // A number in two lists is a precise-range overlap error.
        let overlap_drop = error("=x1~1");
        assert_eq!(
            overlap_drop.message,
            "task 1 cannot both stay in progress and drop in `=x1~1`"
        );
        assert_eq!(overlap_drop.range, (4, 5));

        let overlap_complete_drop = error("=x!2~2");
        assert_eq!(
            overlap_complete_drop.message,
            "task 2 cannot both complete and drop in `=x!2~2`"
        );
        assert_eq!(overlap_complete_drop.range, (5, 6));

        // `~0` names no task, like `!0`.
        let zero_drop = error("=x~0");
        assert_eq!(zero_drop.message, "task numbers start at 1");
        assert_eq!(zero_drop.range, (3, 4));

        // A second `~` points at the second tilde.
        let doubled_tilde = error("=x1~2~3");
        assert_eq!(doubled_tilde.message, "use one `~` list: `=x1~2,3`");
        assert_eq!(doubled_tilde.range, (5, 6));

        // A dangling `~` is an editing state, like `!`.
        let bare_tilde = incomplete("=x~");
        assert_eq!(bare_tilde.in_progress, None);
        assert_eq!(bare_tilde.complete, Vec::<u32>::new());
        assert_eq!(bare_tilde.drop, Vec::<u32>::new());
        assert_eq!(bare_tilde.separator_range, (2, 3));
        assert_eq!(bare_tilde.separator, '~');

        let numbered_tilde = incomplete("=x1~");
        assert_eq!(numbered_tilde.in_progress, Some(vec![1]));
        assert_eq!(numbered_tilde.drop, Vec::<u32>::new());
        assert_eq!(numbered_tilde.separator, '~');

        let complete_tilde = incomplete("=x1!2~");
        assert_eq!(complete_tilde.in_progress, Some(vec![1]));
        assert_eq!(complete_tilde.complete, vec![2]);
        assert_eq!(complete_tilde.complete_range, Some((3, 5)));
        assert_eq!(complete_tilde.drop, Vec::<u32>::new());
        assert_eq!(complete_tilde.separator, '~');

        let drop_comma = incomplete("=x~2,");
        assert_eq!(drop_comma.in_progress, None);
        assert_eq!(drop_comma.drop, vec![2]);
        assert_eq!(drop_comma.drop_range, Some((2, 4)));
        assert_eq!(drop_comma.separator, ',');
    }

    #[test]
    fn lex_reports_dangling_separators_as_incomplete() {
        let trailing_comma = incomplete("=x1,");
        assert_eq!(trailing_comma.in_progress, Some(vec![1]));
        assert_eq!(trailing_comma.complete, Vec::<u32>::new());
        assert_eq!(trailing_comma.in_progress_range, Some((2, 3)));
        assert_eq!(trailing_comma.separator_range, (3, 4));
        assert_eq!(trailing_comma.separator, ',');

        let bare_bang = incomplete("=x!");
        assert_eq!(bare_bang.in_progress, None);
        assert_eq!(bare_bang.complete, Vec::<u32>::new());
        assert_eq!(bare_bang.separator_range, (2, 3));
        assert_eq!(bare_bang.separator, '!');

        let numbered_bang = incomplete("=x1!");
        assert_eq!(numbered_bang.in_progress, Some(vec![1]));
        assert_eq!(numbered_bang.separator, '!');

        let complete_comma = incomplete("=x!2,");
        assert_eq!(complete_comma.in_progress, None);
        assert_eq!(complete_comma.complete, vec![2]);
        assert_eq!(complete_comma.complete_range, Some((2, 4)));
        assert_eq!(complete_comma.separator, ',');

        let none_bang = incomplete("=x0!");
        assert_eq!(none_bang.in_progress, Some(Vec::new()));
        assert_eq!(none_bang.separator, '!');

        // A lexical error elsewhere wins over the incomplete state.
        let duplicate = error("=x1,1,");
        assert_eq!(duplicate.range, (4, 5));
        let empty = error("=x,");
        assert_eq!(empty.message, "expected a task number before `,`");
    }

    #[test]
    fn selection_shape_detection() {
        for token in [
            "=x1", "=X1!2", "=x,", "=x!", "=x~", "=x0", "=x1~2", "=x1!2~3",
        ] {
            assert!(whole_item_close_after_x(token).is_some(), "{token}");
        }
        for token in ["=x", "=X", "=xx", "=xa", "=x.", "=3", "Plan =x1"] {
            assert!(whole_item_close_after_x(token).is_none(), "{token}");
        }
        assert_eq!(link_close_after_x("x1,3!2"), Some("1,3!2"));
        assert_eq!(link_close_after_x("x1~2"), Some("1~2"));
        assert_eq!(link_close_after_x("X1"), Some("1"));
        assert_eq!(link_close_after_x("x"), None);
        assert_eq!(link_close_after_x("xa"), None);
        assert_eq!(link_close_after_x("3"), None);
    }
}
