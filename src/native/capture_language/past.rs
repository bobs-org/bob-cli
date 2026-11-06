//! Shared `x=` past Pomodoro lexer and teaching errors.
//!
//! Both execution (`parse_pomodoro_equals_item`) and the editor
//! (`editor_pomodoro.rs`) consume this module so their lexical results agree.
//! The token split respects quoted exact-name literals: `!` and `=` inside
//! quotes are name content, never repair/close delimiters.

use super::bare_name::{bare_start_name_len, can_spell_bare};
use super::markers::{
    pomodoro_glued_timing_error, pomodoro_named_start_name_error,
    retired_start_spelling_error, POMODORO_START_OVERFLOW_ERROR,
};
use super::pomodoro_literal::{
    canonicalize_exact_name, lex_exact_name_after_hash, opens_exact_name,
};
use super::tokens::is_pomodoro_selector_component;

/// Split of a raw `x=` token (no inline entry, no surrounding whitespace)
/// into its `E` part, optional repair `!`, optional second `=`, and close
/// text. All offsets are relative to the token start (`x=` at 0..2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PastSplit {
    /// Raw `E` text between `x=` and the repair `!` / second `=` / end.
    pub(crate) e_raw: String,
    /// End of `E` within the token (always `2 + e_raw.len()`).
    pub(crate) e_end: usize,
    /// True when a repair `!` precedes the second `=` (or ends the token).
    pub(crate) repair: bool,
    /// Relative offset of the repair `!`, when present.
    pub(crate) repair_offset: Option<usize>,
    /// True when a second `=` was typed.
    pub(crate) has_second_eq: bool,
    /// Relative offset of the second `=`, when present.
    pub(crate) second_eq_offset: Option<usize>,
    /// Text after the second `=` (possibly empty). Empty when no second `=`.
    pub(crate) close_text: String,
    /// Relative offset where `close_text` starts (equals `len` when absent).
    pub(crate) close_offset: usize,
    /// Full token length.
    pub(crate) len: usize,
}

/// Split `raw_token` (which starts with `x=` and contains no inline entry)
/// into `E`, repair, second `=`, and close parts, ignoring `!`/`=` inside a
/// quoted exact-name literal.
pub(crate) fn split_past_token(raw_token: &str) -> Option<PastSplit> {
    if !raw_token.starts_with("x=") {
        return None;
    }
    // Locate a quoted literal so `!`/`=` inside quotes stay name content.
    let mut literal_range: Option<(usize, usize)> = None;
    if let Some(hash_pos) = raw_token.find('#') {
        // Only the first `#` can open the literal (`D#lit` or `#lit`).
        if hash_pos >= 2 {
            let after_hash = &raw_token[hash_pos + 1..];
            if opens_exact_name(after_hash)
                && let Some(lex) =
                    lex_exact_name_after_hash(after_hash, after_hash.len())
            {
                literal_range =
                    Some((hash_pos, hash_pos + 1 + lex.literal_len));
            }
        }
    }
    let in_literal = |pos: usize| -> bool {
        literal_range.is_some_and(|(start, end)| pos >= start && pos < end)
    };
    // Second `=` is the first `=` after `x=` outside the literal.
    let mut second_eq: Option<usize> = None;
    for (offset, byte) in raw_token.bytes().enumerate().skip(2) {
        if byte == b'=' && !in_literal(offset) {
            second_eq = Some(offset);
            break;
        }
    }
    let (left_end, has_second_eq, second_eq_offset, close_text, close_offset) =
        match second_eq {
            Some(pos) => {
                let close = raw_token[pos + 1..].to_string();
                (pos, true, Some(pos), close, pos + 1)
            }
            None => {
                let len = raw_token.len();
                (len, false, None, String::new(), len)
            }
        };
    // Repair `!` is a trailing `!` of the left part, outside the literal.
    let mut repair = false;
    let mut repair_offset = None;
    if left_end > 2 {
        let candidate = left_end - 1;
        if raw_token.as_bytes()[candidate] == b'!' && !in_literal(candidate) {
            repair = true;
            repair_offset = Some(candidate);
        }
    }
    let e_end = if repair { left_end - 1 } else { left_end };
    let e_raw = raw_token.get(2..e_end).unwrap_or("").to_string();
    Some(PastSplit {
        e_raw,
        e_end,
        repair,
        repair_offset,
        has_second_eq,
        second_eq_offset,
        close_text,
        close_offset,
        len: raw_token.len(),
    })
}

/// Validated `E` part: block count, optional typed name, and optional
/// segment suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PastEValid {
    /// Block count (default 5 when untyped).
    pub(crate) blocks: u32,
    /// True when a count was typed (via `D` or `@D`).
    pub(crate) blocks_typed: bool,
    /// Canonical upper-cased name, when typed.
    pub(crate) name: Option<String>,
    /// True when the name came from a quoted literal.
    pub(crate) name_is_literal: bool,
    /// Relative range of the leading `D` digits, when typed.
    pub(crate) blocks_range: Option<(usize, usize)>,
    /// Relative range of the name (without `#`/`@D`), when typed.
    pub(crate) name_range: Option<(usize, usize)>,
    /// Validated segment suffix, when typed.
    pub(crate) segment: Option<super::PastSegment>,
    /// Relative range of the suffix (`~` through an optional `+`), when typed.
    pub(crate) segment_range: Option<(usize, usize)>,
}

/// What `validate_past_e` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PastEOutcome {
    Valid(PastEValid),
    /// Unfinished input: the editor needs `need`, execution reports the
    /// matching incomplete error. `message` is the execution message.
    Incomplete {
        need: PastNeed,
        message: String,
    },
    /// Malformed `E`: teaching error message for execution (the editor
    /// reports the same message with code `invalid_pomodoro_past`).
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PastNeed {
    Name,
}

/// Validate the `E` part of a past token. `raw_token` is the full past token
/// (named in every teaching error); `split` is its [`split_past_token`]
/// result. Ranges are relative to the token start.
///
/// `E` is scanned left to right: an optional leading count `N`, then a `#`
/// name or literal or a bare name with an optional trailing count `N`
/// (`x=3fix`, `x=fix3`). `@D` is retired: timing glues to the name
/// (`x=fix@3` teaches `x=fix3`). A dash after a bare name starts a
/// teaching error, never a name byte. Every rewrite is built from the typed
/// parts and keeps the typed repair `!` and close part.
pub(crate) fn validate_past_e(
    raw_token: &str,
    split: &PastSplit,
) -> PastEOutcome {
    let e = split.e_raw.as_str();
    // Empty `E` is the default 25-minute unnamed session.
    if e.is_empty() {
        return PastEOutcome::Valid(PastEValid {
            blocks: 5,
            blocks_typed: false,
            name: None,
            name_is_literal: false,
            blocks_range: None,
            name_range: None,
            segment: None,
            segment_range: None,
        });
    }
    // Split/merge shapes never record a past session.
    if e.starts_with('/')
        || e.strip_prefix('*')
            .is_some_and(|after| !is_task_number_list(after))
    {
        return PastEOutcome::Invalid(format!(
            "`{raw_token}` records one past session; it can't split (`=/`) or merge (`=*`)"
        ));
    }
    // Leading `N` block count (past takes no offset).
    let d_end = e.bytes().take_while(u8::is_ascii_digit).count();
    let d_text = &e[..d_end];
    let mut pos = d_end;
    // `#` name or literal, or a bare name with an optional trailing count.
    let mut name: Option<PastName> = None;
    let mut trailing: Option<(usize, usize)> = None;
    if e[pos..].starts_with('#') {
        match lex_past_hash_name(raw_token, e, pos) {
            Ok(lexed) => {
                // Retire `#` before names a bare spelling can express.
                // A following `@` is handled below so the count is kept
                // (`x=#fix@3` → `x=fix3`); a `~` segment suffix is preserved.
                if !lexed.is_literal {
                    let typed = &e[lexed.start..lexed.end];
                    if can_spell_bare(typed) {
                        let tail = &e[lexed.end..];
                        if !tail.starts_with('@') {
                            let kept_tail =
                                if tail.starts_with('~') { tail } else { "" };
                            let rewrite = format!(
                                "x={d_text}{typed}{}{}",
                                kept_tail,
                                past_suffix(split)
                            );
                            return PastEOutcome::Invalid(
                                retired_start_spelling_error(
                                    raw_token,
                                    "`#` is only for names a bare spelling cannot express",
                                    &rewrite,
                                ),
                            );
                        }
                    }
                }
                pos = lexed.end;
                name = Some(lexed);
            }
            Err(outcome) => return outcome,
        }
    } else if let Some(name_len) = bare_start_name_len(&e[pos..]) {
        // Reject bytes `=name` rejects before peeling the trailing count,
        // so `x=a$b` and `x=build#2` keep the start-family wording.
        let full = past_e_name_extent(&e[pos..]);
        let full_text = &e[pos..pos + full];
        if !is_pomodoro_selector_component(full_text) {
            return PastEOutcome::Invalid(pomodoro_named_start_name_error(
                full_text, raw_token,
            ));
        }
        let text = &e[pos..pos + name_len];
        name = Some(PastName {
            canonical: text.to_ascii_uppercase(),
            is_literal: false,
            start: pos,
            end: pos + name_len,
        });
        pos += name_len;
        // A trailing `N` block count glues to the bare name.
        let digits = e[pos..].bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            trailing = Some((pos, pos + digits));
            pos += digits;
        }
    }
    // Retired `@D`: timing glues to the name now.
    if let Some(after_at) = e[pos..].strip_prefix('@') {
        let digits = after_at.bytes().take_while(u8::is_ascii_digit).count();
        let at_count = if digits == 0 {
            "3"
        } else if after_at[..digits].trim_start_matches('0').is_empty() {
            "1"
        } else {
            &after_at[..digits]
        };
        let tail = &after_at[digits..];
        // A `~` segment suffix stays on `E`; other tails (`*`) move to close.
        let (kept_e_tail, moved_close) = if tail.starts_with('~') {
            (tail, String::new())
        } else if !tail.is_empty() {
            ("", format!("={tail}"))
        } else {
            ("", String::new())
        };
        let raw_name = name
            .as_ref()
            .map(|lexed| &e[lexed.start..lexed.end])
            .unwrap_or("");
        let is_literal = name.as_ref().is_some_and(|lexed| lexed.is_literal);
        let suffix = past_suffix(split);
        if d_end > 0 {
            let leading = &e[..d_end];
            let keep_leading = format!(
                "x={}{kept_e_tail}{moved_close}{suffix}",
                compose_past_e(leading, raw_name, "", is_literal)
            );
            let keep_trailing = format!(
                "x={}{kept_e_tail}{moved_close}{suffix}",
                compose_past_e("", raw_name, at_count, is_literal)
            );
            return PastEOutcome::Invalid(format!(
                "`{raw_token}` gives the timing twice; keep one: `{keep_leading}` or `{keep_trailing}`"
            ));
        }
        let glued = compose_past_e("", raw_name, at_count, is_literal);
        let rewrite = format!("x={glued}{kept_e_tail}{moved_close}{suffix}");
        return PastEOutcome::Invalid(retired_start_spelling_error(
            raw_token,
            "`@` no longer separates a name from its timing",
            &rewrite,
        ));
    }
    // An optional segment suffix closes `E`: it must consume the rest.
    let mut segment: Option<super::PastSegment> = None;
    let mut segment_range: Option<(usize, usize)> = None;
    {
        let rest = &e[pos..];
        if rest.starts_with('~') {
            let name_is_bare = name.as_ref().is_some_and(|n| !n.is_literal);
            let bare_name_text = if name_is_bare {
                name.as_ref().map(|n| &e[n.start..n.end])
            } else {
                None
            };
            match lex_past_suffix(
                raw_token,
                e,
                pos,
                split,
                name_is_bare,
                bare_name_text,
            ) {
                Ok((seg, len)) => {
                    segment = Some(seg);
                    segment_range = Some((2 + pos, 2 + pos + len));
                    pos += len;
                }
                Err(message) => return PastEOutcome::Invalid(message),
            }
        }
    }
    // Anything left in `E` is a teaching error.
    let rest = &e[pos..];
    if !rest.is_empty() {
        return PastEOutcome::Invalid(past_rest_error(
            raw_token,
            split,
            &e[..pos],
            rest,
            PastRestContext {
                has_leading: d_end > 0,
                has_name: name.is_some(),
                name_is_bare: name.as_ref().is_some_and(|n| !n.is_literal),
            },
        ));
    }
    let suffix = past_suffix(split);
    if d_end > 0
        && let Some((trail_start, trail_end)) = trailing
    {
        // `x=3fix2`: blocks given twice.
        let raw_name = name
            .as_ref()
            .map(|lexed| &e[lexed.start..lexed.end])
            .unwrap_or("");
        let leading = &e[..d_end];
        let trailing_text = &e[trail_start..trail_end];
        return PastEOutcome::Invalid(format!(
            "`{raw_token}` gives the timing twice; keep one: `x={leading}{raw_name}{suffix}` or `x={raw_name}{trailing_text}{suffix}`"
        ));
    }
    // Effective block count: the leading `N` or the trailing `N`.
    let count_range = trailing.or((d_end > 0).then_some((0, d_end)));
    let blocks = match count_range {
        None => 5,
        Some((start, end)) => match e[start..end].parse::<u64>() {
            Ok(0) => {
                let rewrite =
                    format!("x={}1{}{suffix}", &e[..start], &e[end..]);
                return PastEOutcome::Invalid(format!(
                    "a past Pomodoro needs at least one block: `{raw_token}` records nothing, write `{rewrite}`"
                ));
            }
            Ok(value) => match u32::try_from(value) {
                Ok(blocks) => blocks,
                Err(_) => {
                    return PastEOutcome::Invalid(
                        POMODORO_START_OVERFLOW_ERROR.to_string(),
                    );
                }
            },
            Err(_) => {
                return PastEOutcome::Invalid(
                    POMODORO_START_OVERFLOW_ERROR.to_string(),
                );
            }
        },
    };
    let (name, name_is_literal, name_range) = match name {
        Some(lexed) => (
            Some(lexed.canonical),
            lexed.is_literal,
            Some((2 + lexed.start, 2 + lexed.end)),
        ),
        None => (None, false, None),
    };
    PastEOutcome::Valid(PastEValid {
        blocks,
        blocks_typed: count_range.is_some(),
        name,
        name_is_literal,
        blocks_range: count_range.map(|(start, end)| (2 + start, 2 + end)),
        name_range,
        segment,
        segment_range,
    })
}

/// A lexed past name. `start..end` is relative to `E`: the name only for a
/// bare or `#` name, the quoted literal (both quotes) for a literal.
struct PastName {
    canonical: String,
    is_literal: bool,
    start: usize,
    end: usize,
}

/// Lex the `#` name or literal at `hash_pos` in `e`. Literals canonicalize
/// like `=#"…"`; plain names keep the `=name` byte rules.
fn lex_past_hash_name(
    raw_token: &str,
    e: &str,
    hash_pos: usize,
) -> Result<PastName, PastEOutcome> {
    let needs_name = || PastEOutcome::Incomplete {
        need: PastNeed::Name,
        message: format!("`{raw_token}` needs a Pomodoro name after `#`"),
    };
    let start = hash_pos + 1;
    let after_hash = &e[start..];
    if opens_exact_name(after_hash) {
        let Some(lex) = lex_exact_name_after_hash(after_hash, after_hash.len())
        else {
            return Err(needs_name());
        };
        if !lex.complete {
            return Err(needs_name());
        }
        let Some(canonical) = canonicalize_exact_name(&lex.decoded) else {
            return Err(PastEOutcome::Invalid(
                pomodoro_named_start_name_error(&lex.decoded, raw_token),
            ));
        };
        return Ok(PastName {
            canonical,
            is_literal: true,
            start,
            end: start + lex.literal_len,
        });
    }
    let len = past_e_name_extent(after_hash);
    if len == 0 {
        return Err(needs_name());
    }
    let text = &after_hash[..len];
    if !is_pomodoro_selector_component(text) {
        return Err(PastEOutcome::Invalid(pomodoro_named_start_name_error(
            text, raw_token,
        )));
    }
    Ok(PastName {
        canonical: text.to_ascii_uppercase(),
        is_literal: false,
        start,
        end: start + len,
    })
}

/// Byte length of a past name inside `E`: the shared past extent, also
/// stopping at `*` so a glued park list (`x=build*3`) teaches its rewrite.
fn past_e_name_extent(text: &str) -> usize {
    let extent = super::item::past_name_extent(text);
    text[..extent].find('*').unwrap_or(extent)
}

/// What `E` held before its unparsed rest.
#[derive(Clone, Copy)]
struct PastRestContext {
    has_leading: bool,
    has_name: bool,
    name_is_bare: bool,
}

/// Lex the segment suffix at `pos` in `e` (`e[pos..]` starts with `~`).
/// Returns the validated segment plus its byte length in `e`. The suffix
/// must consume the rest of `E`; any leftover produces its own teaching
/// error. `name_is_bare` and `bare_name_text` support the `deep~work`
/// dashed-name teaching. All rewrites keep the typed repair `!` and close
/// part via [`past_suffix`].
fn lex_past_suffix(
    raw_token: &str,
    e: &str,
    pos: usize,
    split: &PastSplit,
    name_is_bare: bool,
    bare_name_text: Option<&str>,
) -> Result<(super::PastSegment, usize), String> {
    use super::{PastEdge, PastSegment, PastSegmentKind};
    let rest = &e[pos..];
    let parsed_prefix = &e[..pos];
    let repair_close = past_suffix(split);
    let mk_tail = |raw: &str| PastSegment {
        raw: raw.to_string(),
        kind: PastSegmentKind::Tail,
        index: None,
        edge: PastEdge::Start,
    };
    let mk_head = |raw: &str| PastSegment {
        raw: raw.to_string(),
        kind: PastSegmentKind::Head,
        index: None,
        edge: PastEdge::End,
    };
    let mk_index = |raw: &str, index: u32, edge: PastEdge| PastSegment {
        raw: raw.to_string(),
        kind: PastSegmentKind::Index,
        index: Some(index),
        edge,
    };
    // `~~` head first (longest match).
    if let Some(after) = rest.strip_prefix("~~") {
        if after.is_empty() {
            return Ok((mk_head("~~"), 2));
        }
        // Extra after `~~`.
        if after.starts_with('~') {
            return Err(format!(
                "`{raw_token}` has two segment suffixes; one segment suffix per item"
            ));
        }
        // A name after the suffix belongs before it.
        if after
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'#')
        {
            let new_e = format!("{parsed_prefix}{after}~~");
            return Err(format!(
                "the suffix comes last in `E`: `{raw_token}` places it before the name, write `x={new_e}{repair_close}`"
            ));
        }
        // `~~` plus a number: no number allowed — unless a count/name
        // already precedes it, in which case the digit starts a second
        // suffix attempt (`x=2~~1`).
        if after.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            if !parsed_prefix.is_empty() {
                return Err(format!(
                    "`{raw_token}` has two segment suffixes; one segment suffix per item"
                ));
            }
            let rewrite = format!("x={parsed_prefix}~~{repair_close}");
            return Err(format!(
                "`~~` already ends at your first Pomodoro and takes no number: `{raw_token}` ends there, write `{rewrite}`"
            ));
        }
        if after.starts_with('+') || after.starts_with(',') {
            // `~~+` takes no number; `~~,3` is not a suffix shape.
            if after.starts_with('+') && !parsed_prefix.is_empty() {
                return Err(format!(
                    "`{raw_token}` has two segment suffixes; one segment suffix per item"
                ));
            }
            let rewrite = format!("x={parsed_prefix}~~{repair_close}");
            return Err(format!(
                "`~~` already ends at your first Pomodoro and takes no number: `{raw_token}` ends there, write `{rewrite}`"
            ));
        }
        // Any other extra (`!`, `*`, etc.) after `~~` takes no number.
        let rewrite = format!("x={parsed_prefix}~~{repair_close}");
        return Err(format!(
            "`~~` already ends at your first Pomodoro and takes no number: `{raw_token}` ends there, write `{rewrite}`"
        ));
    }
    // Single `~`: tail, indexed, or an error.
    let after = &rest[1..];
    if after.is_empty() {
        return Ok((mk_tail("~"), 1));
    }
    // `~` followed by letters or `#`: either the dashed-name teaching or
    // a tail suffix with the name after it.
    if after
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'#')
    {
        // `x=deep~work`: a bare name, `~`, letters, and a valid
        // `deep-work` selector teaches `#`.
        if name_is_bare
            && let Some(bare) = bare_name_text
            && let Some(tilde_end) = after
                .find(|c: char| {
                    !(c.is_ascii_alphanumeric() || c == '-' || c == '_')
                })
                .or(Some(after.len()))
            && tilde_end > 0
        {
            let after_name = &after[..tilde_end];
            let full_name = format!("{bare}-{after_name}");
            if is_pomodoro_selector_component(&full_name) {
                let leading = parsed_prefix
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count()
                    .min(parsed_prefix.len());
                let (leading_text, _) = parsed_prefix.split_at(leading);
                let hash_form = format!("x={leading_text}#{full_name}");
                // Only teach the hash form when nothing else follows the
                // name (a count or suffix after would be a different error).
                let tail = &after[tilde_end..];
                if tail.is_empty() {
                    return Err(pomodoro_glued_timing_error(
                        raw_token, &hash_form,
                    ));
                }
            }
        }
        // Otherwise the tail suffix comes last.
        let new_e = format!("{parsed_prefix}{after}~");
        return Err(format!(
            "the suffix comes last in `E`: `{raw_token}` places it before the name, write `x={new_e}{repair_close}`"
        ));
    }
    if let Some(plus_tail) = after.strip_prefix('+') {
        // `~+` without an index: two readings.
        let end_now = format!("x={parsed_prefix}{repair_close}");
        let fill = format!("x={parsed_prefix}~{repair_close}");
        let _ = plus_tail;
        return Err(format!(
            "`{raw_token}` takes no offset; to end now write `{end_now}`, to fill since your last Pomodoro write `{fill}`"
        ));
    }
    if after.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        let digit_len = after.bytes().take_while(u8::is_ascii_digit).count();
        let digits = &after[..digit_len];
        let after_digits = &after[digit_len..];
        // Three-or-more-digit indexes are reserved for a future clock,
        // even with a leading zero (`~0800`).
        if digit_len >= 3 {
            return Err(format!(
                "`{raw_token}` reserves three-or-more-digit suffixes for a future clock suffix; segments are 1-99"
            ));
        }
        // Leading-zero and zero shapes.
        if digits.starts_with('0') {
            if digits == "0" && after_digits.is_empty() {
                let rewrite = format!("x={parsed_prefix}~{repair_close}");
                return Err(format!(
                    "`~` already names the tail: `{raw_token}` starts where your last Pomodoro ended, write `{rewrite}`"
                ));
            }
            if digits == "0" && after_digits.starts_with('+') {
                // `~0+`: like `~+`.
                let end_now = format!("x={parsed_prefix}{repair_close}");
                let fill = format!("x={parsed_prefix}~{repair_close}");
                return Err(format!(
                    "`{raw_token}` takes no offset; to end now write `{end_now}`, to fill since your last Pomodoro write `{fill}`"
                ));
            }
            if digits.len() > 1 {
                // `~01`: strip the leading zero.
                let stripped = digits.trim_start_matches('0');
                let index_text =
                    if stripped.is_empty() { "0" } else { stripped };
                // Preserve any `+` and complain about the zero.
                let plus = if after_digits.starts_with('+') {
                    "+"
                } else {
                    ""
                };
                let rewrite = format!(
                    "x={parsed_prefix}~{index_text}{plus}{repair_close}"
                );
                return Err(format!(
                    "`{raw_token}` has a leading zero: write `{rewrite}`"
                ));
            }
            // Remaining `~0...` shapes name the tail.
            let leftover = after_digits;
            let new_e = if leftover.is_empty() {
                format!("{parsed_prefix}~")
            } else {
                format!("{parsed_prefix}{leftover}~")
            };
            let rewrite = format!("x={new_e}{repair_close}");
            return Err(format!(
                "`~` already names the tail: `{raw_token}` starts where your last Pomodoro ended, write `{rewrite}`"
            ));
        }
        let index: u32 = digits.parse().unwrap_or(0);
        if after_digits.is_empty() {
            let raw = format!("~{digits}");
            let len = 1 + digit_len;
            return Ok((mk_index(&raw, index, PastEdge::Start), len));
        }
        if let Some(after_plus) = after_digits.strip_prefix('+') {
            let raw = format!("~{digits}+");
            let len = 1 + digit_len + 1;
            if after_plus.is_empty() {
                return Ok((mk_index(&raw, index, PastEdge::End), len));
            }
            // Extra after `~n+`.
            if after_plus.starts_with('~') {
                return Err(format!(
                    "`{raw_token}` has two segment suffixes; one segment suffix per item"
                ));
            }
            if after_plus.contains(',') {
                let moved = format!("{raw}{after_plus}");
                let rewrite = past_close_rewrite(parsed_prefix, &moved, split);
                return Err(format!(
                    "a comma list is a drop: `{raw_token}` drops links, write `{rewrite}`"
                ));
            }
            if after_plus
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'#')
            {
                let new_e = format!("{parsed_prefix}{after_plus}{raw}");
                return Err(format!(
                    "the suffix comes last in `E`: `{raw_token}` places it before the name, write `x={new_e}{repair_close}`"
                ));
            }
            // `!`, `*`, or other bytes after `~n+`.
            let parsed_with_suffix = format!("{parsed_prefix}{raw}");
            return Err(past_rest_error(
                raw_token,
                split,
                &parsed_with_suffix,
                after_plus,
                PastRestContext {
                    has_leading: !parsed_prefix.is_empty(),
                    has_name: true,
                    name_is_bare: false,
                },
            ));
        }
        // Extra after `~n` without `+`.
        if after_digits.starts_with('~') {
            return Err(format!(
                "`{raw_token}` has two segment suffixes; one segment suffix per item"
            ));
        }
        if after_digits.contains(',') {
            let moved = format!("~{digits}{after_digits}");
            let rewrite = past_close_rewrite(parsed_prefix, &moved, split);
            return Err(format!(
                "a comma list is a drop: `{raw_token}` drops links, write `{rewrite}`"
            ));
        }
        if after_digits
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'#')
        {
            let raw = format!("~{digits}");
            let new_e = format!("{parsed_prefix}{after_digits}{raw}");
            return Err(format!(
                "the suffix comes last in `E`: `{raw_token}` places it before the name, write `x={new_e}{repair_close}`"
            ));
        }
        if let Some(bang_plus) = after_digits.strip_prefix('!') {
            // `~n!+`: the `+` belongs before the repair `!`.
            if bang_plus.starts_with('+') {
                let raw = format!("~{digits}+");
                let rewrite = format!(
                    "x={parsed_prefix}{raw}!{repair_close_trimmed}",
                    repair_close_trimmed = repair_close
                        .strip_prefix('!')
                        .unwrap_or(repair_close.as_str()),
                );
                // When the original already had a repair `!`, the trimmed
                // close keeps it exactly once.
                let rewrite = if split.repair {
                    format!(
                        "x={parsed_prefix}~{digits}+!{close_tail}",
                        close_tail = if split.has_second_eq {
                            format!("={}", split.close_text)
                        } else {
                            String::new()
                        }
                    )
                } else {
                    rewrite
                };
                return Err(format!(
                    "`+` belongs to the suffix, before `!`: `{raw_token}` ends at the segment edge, write `{rewrite}`"
                ));
            }
        }
        // `!`, `*`, or other bytes after `~n`: reuse the generic teaching
        // with the suffix included in the parsed prefix.
        let parsed_with_suffix = format!("{parsed_prefix}~{digits}");
        return Err(past_rest_error(
            raw_token,
            split,
            &parsed_with_suffix,
            after_digits,
            PastRestContext {
                has_leading: !parsed_prefix.is_empty(),
                has_name: true,
                name_is_bare: false,
            },
        ));
    }
    // Any other byte after `~` (`,`, `!`, `*`, `-`, ...): the tail suffix
    // plus leftover, handled by the generic teaching with the suffix
    // included — except a comma list, which is a drop.
    if after.contains(',') {
        let moved = format!("~{after}");
        let rewrite = past_close_rewrite(parsed_prefix, &moved, split);
        return Err(format!(
            "a comma list is a drop: `{raw_token}` drops links, write `{rewrite}`"
        ));
    }
    let parsed_with_suffix = format!("{parsed_prefix}~");
    Err(past_rest_error(
        raw_token,
        split,
        &parsed_with_suffix,
        after,
        PastRestContext {
            has_leading: !parsed_prefix.is_empty(),
            has_name: true,
            name_is_bare: false,
        },
    ))
}

/// Teaching error for text left in `E` after its count/name prefix
/// (`parsed`). Close-part text moves after a second `=`.
fn past_rest_error(
    raw_token: &str,
    split: &PastSplit,
    parsed: &str,
    rest: &str,
    context: PastRestContext,
) -> String {
    let suffix = past_suffix(split);
    if let Some(after_dash) = rest.strip_prefix('-') {
        // A dash continued into letters is a bare-name spelling that must
        // use `#`; any other dash is an offset, which past work never takes.
        if context.name_is_bare
            && after_dash
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
        {
            let leading = if context.has_leading {
                parsed
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count()
                    .min(parsed.len())
            } else {
                0
            };
            let (leading_text, name_text) = parsed.split_at(leading);
            let full_name = format!("{name_text}-{after_dash}");
            if is_pomodoro_selector_component(&full_name) {
                let hash_form = format!("x={leading_text}#{full_name}");
                return pomodoro_glued_timing_error(raw_token, &hash_form);
            }
        }
        // Dash offsets teach two readings: the segment suffix and ending now.
        let digit_len =
            after_dash.bytes().take_while(u8::is_ascii_digit).count();
        let digits = &after_dash[..digit_len];
        let kept = &after_dash[digit_len..];
        let end_now = format!("x={parsed}{kept}{suffix}");
        if digits.is_empty() {
            let fill = format!("x={parsed}~{suffix}");
            return format!(
                "`{raw_token}` takes no offset; to start where your last Pomodoro ended write `{fill}`, to end now write `{end_now}`"
            );
        }
        let fill = format!("x={parsed}{kept}~{digits}{suffix}");
        return format!(
            "`{raw_token}` takes no offset; to start at segment {digits} write `{fill}`, to end now write `{end_now}`"
        );
    }
    if let Some(list) = rest.strip_prefix('*') {
        let rewrite = past_close_rewrite(parsed, rest, split);
        let tasks = match list {
            "" => "every task".to_string(),
            _ if list.contains(',') => format!("tasks {list}"),
            _ => format!("task {list}"),
        };
        return format!(
            "to park {tasks}, put it after a second `=`: `{raw_token}` never parks, write `{rewrite}`"
        );
    }
    if rest.starts_with('~') && rest.contains(',') {
        let rewrite = past_close_rewrite(parsed, rest, split);
        return format!(
            "a comma list is a drop: `{raw_token}` drops links, write `{rewrite}`"
        );
    }
    if rest.starts_with('!') {
        if !context.has_name {
            // A bare count before `!` is part of the selection: `x=1!2`
            // keeps task 1 and completes task 2.
            let rewrite = past_close_rewrite("", &split.e_raw, split);
            return format!(
                "write the task selection after a second `=`: `{raw_token}` selects tasks, write `{rewrite}`"
            );
        }
        let rewrite = past_close_rewrite(parsed, rest, split);
        let repair_hint = if split.repair {
            String::new()
        } else {
            let mut repaired = split.clone();
            repaired.repair = true;
            format!(
                "; to also repair, write `{}`",
                past_close_rewrite(parsed, rest, &repaired)
            )
        };
        return format!(
            "write the task selection after a second `=`: `{raw_token}` selects tasks, write `{rewrite}`{repair_hint}"
        );
    }
    if context.has_leading
        && !context.has_name
        && rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
    {
        let rewrite = format!("x={parsed}#{rest}{suffix}");
        return format!(
            "write `#` between the count and the name: `{raw_token}` counts blocks, write `{rewrite}`"
        );
    }
    format!("`{raw_token}` is not a past Pomodoro")
}

/// Compose the `E` spelling for a past rewrite: bare names glue timing on
/// either side; non-bare names and literals put it before `#`.
fn compose_past_e(
    leading: &str,
    name: &str,
    trailing: &str,
    is_literal: bool,
) -> String {
    if name.is_empty() {
        return format!("{leading}{trailing}");
    }
    if is_literal {
        let timing = if leading.is_empty() {
            trailing
        } else {
            leading
        };
        return format!("{timing}#{name}");
    }
    if can_spell_bare(name) {
        if !leading.is_empty() {
            format!("{leading}{name}")
        } else {
            format!("{name}{trailing}")
        }
    } else {
        let timing = if leading.is_empty() {
            trailing
        } else {
            leading
        };
        format!("{timing}#{name}")
    }
}

/// The typed repair `!` and second-`=` close part, as they follow `E`.
fn past_suffix(split: &PastSplit) -> String {
    let mut suffix = String::new();
    if split.repair {
        suffix.push('!');
    }
    if split.has_second_eq {
        suffix.push('=');
        suffix.push_str(&split.close_text);
    }
    suffix
}

/// Rewrite that keeps `parsed` as `E` and moves `moved` into the close part
/// after a second `=`, merged with any typed close text (a leading count
/// stays first).
fn past_close_rewrite(parsed: &str, moved: &str, split: &PastSplit) -> String {
    let bang = if split.repair { "!" } else { "" };
    let close = split.close_text.as_str();
    let combined = if moved.starts_with(|c: char| c.is_ascii_digit()) {
        format!("{moved}{close}")
    } else {
        format!("{close}{moved}")
    };
    format!("x={parsed}{bang}={combined}")
}

/// Whether `text` is a nonempty comma-separated task-number list (`3`,
/// `1,2`), as typed after a park `*`.
fn is_task_number_list(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit() || b == b',')
}

/// One member's contribution to [`compute_past_stacks`]: its block count
/// (when valid), its suffix edge and raw suffix (when a suffix was typed),
/// and its full raw token for pin-rewrite diagnostics.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PastStackMember<'a> {
    pub(crate) blocks: Option<u32>,
    pub(crate) edge: Option<super::PastEdge>,
    pub(crate) segment_raw: Option<&'a str>,
    pub(crate) raw_token: &'a str,
}

/// Compute stack positions plus pin diagnostics for a run of past items.
/// `members[i].blocks` is `Some` for a valid Past item, `None` for every
/// other item (which breaks runs). Returns positions plus `(item index,
/// message)` pin diagnostics using the two run messages in the contract.
/// A start suffix (`~`, `~n`) belongs on the run's first item; an end
/// suffix (`~n+`, `~~`) belongs on its last. A suffix anywhere else, or a
/// second suffix in one run, is reported on the offending item.
pub(crate) fn compute_past_stacks(
    members: &[PastStackMember<'_>],
) -> (Vec<super::PastStackPos>, Vec<(usize, String)>) {
    use super::{PastEdge, PastStackPos};
    let zero = PastStackPos {
        index: 0,
        size: 0,
        offset_blocks: 0,
        pin: None,
        lead_blocks: None,
    };
    let mut out = vec![zero; members.len()];
    let mut diagnostics = Vec::new();
    let mut run_start: Option<usize> = None;
    for (i, m) in members.iter().enumerate() {
        let is_past = m.blocks.is_some();
        let run_end = if is_past {
            if i + 1 == members.len() || members[i + 1].blocks.is_none() {
                Some(i)
            } else {
                None
            }
        } else {
            None
        };
        if is_past && run_start.is_none() {
            run_start = Some(i);
        }
        if let (Some(start), Some(end)) = (run_start, run_end) {
            let size = (end - start + 1) as u32;
            // Suffix sums with a wide representation.
            let mut suffix: u64 = 0;
            for k in (start..=end).rev() {
                let blk = members[k].blocks.unwrap_or(0) as u64;
                out[k] = PastStackPos {
                    index: (k - start + 1) as u32,
                    size,
                    offset_blocks: u32::try_from(suffix).unwrap_or(u32::MAX),
                    pin: None,
                    lead_blocks: None,
                };
                suffix = suffix.saturating_add(blk);
            }
            // Pin assignment and diagnostics for this run.
            let suffixed: Vec<usize> = (start..=end)
                .filter(|k| members[*k].edge.is_some())
                .collect();
            if suffixed.is_empty() {
                // Unpinned run: positions already set.
            } else if suffixed.len() == 1 {
                let k = suffixed[0];
                let edge = members[k].edge.unwrap_or(PastEdge::Start);
                let legal = (edge == PastEdge::Start && k == start)
                    || (edge == PastEdge::End && k == end);
                if legal {
                    // Prefix sums for lead_blocks.
                    let mut lead: u64 = 0;
                    for m_idx in start..=end {
                        let lead_blocks =
                            u32::try_from(lead).unwrap_or(u32::MAX);
                        out[m_idx].pin = Some(edge);
                        out[m_idx].lead_blocks = Some(lead_blocks);
                        lead = lead.saturating_add(u64::from(
                            members[m_idx].blocks.unwrap_or(0),
                        ));
                    }
                } else {
                    diagnostics.push((
                        k,
                        misplaced_pin_message(members, start, end, k),
                    ));
                }
            } else {
                // Two or more suffixes: the first keeps its position for
                // context, every later suffixed member adds a second place.
                for (n, k) in suffixed.iter().enumerate() {
                    if n == 0 {
                        continue;
                    }
                    let raw = members[*k].raw_token;
                    diagnostics.push((
                        *k,
                        format!(
                            "`{raw}` adds a second place to this run: a run of `x=` items hangs from one place; keep one suffix or capture them separately"
                        ),
                    ));
                }
                // Also report a misplaced first suffix, so a run like
                // `x=a x=b~2 x=c~1` names both problems.
                let first = suffixed[0];
                let edge = members[first].edge.unwrap_or(PastEdge::Start);
                let legal = (edge == PastEdge::Start && first == start)
                    || (edge == PastEdge::End && first == end);
                if !legal {
                    diagnostics.push((
                        first,
                        misplaced_pin_message(members, start, end, first),
                    ));
                }
            }
            run_start = None;
        }
        if !is_past {
            run_start = None;
        }
    }
    // Stable diagnostic order by item index.
    diagnostics.sort_by_key(|(idx, _)| *idx);
    (out, diagnostics)
}

/// Misplaced-suffix message with a rewrite that moves the suffix text to
/// the legal end member (first for a start pin, last for an end pin).
fn misplaced_pin_message(
    members: &[PastStackMember<'_>],
    start: usize,
    end: usize,
    offending: usize,
) -> String {
    use super::PastEdge;
    let raw = members[offending].raw_token;
    let edge = members[offending].edge.unwrap_or(PastEdge::Start);
    let target = if edge == PastEdge::Start { start } else { end };
    let rewrite =
        rewrite_run_with_moved_suffix(members, start, end, offending, target);
    format!(
        "`{raw}` can't place this run: a run of `x=` items hangs from one place, so a start suffix (`~`, `~n`) goes on its first item and an end suffix (`~n+`, `~~`) on its last; write `{rewrite}` or capture them separately"
    )
}

/// Rebuild a run's tokens with the offending member's suffix moved to
/// `target` (the first member for a start pin, the last for an end pin).
/// Every other member keeps its typed repair `!` and close part; only the
/// suffix bytes move.
fn rewrite_run_with_moved_suffix(
    members: &[PastStackMember<'_>],
    start: usize,
    end: usize,
    offending: usize,
    target: usize,
) -> String {
    let suffix_raw = members[offending].segment_raw.unwrap_or("");
    let mut tokens = Vec::new();
    for idx in start..=end {
        let raw = members[idx].raw_token;
        if idx == offending {
            tokens.push(remove_suffix_text(raw, suffix_raw));
        } else if idx == target {
            tokens.push(insert_suffix_text(raw, suffix_raw));
        } else {
            tokens.push(raw.to_string());
        }
    }
    tokens.join(" ")
}

/// Remove one occurrence of the suffix bytes from a raw past token (the
/// suffix inside `E`, before any repair `!` or second `=`).
fn remove_suffix_text(raw: &str, suffix_raw: &str) -> String {
    if suffix_raw.is_empty() {
        return raw.to_string();
    }
    if let Some(split) = split_past_token(raw) {
        // The suffix ends at `e_end` for a valid suffix with no leftover.
        let suffix_len = suffix_raw.len();
        if split.e_end >= suffix_len {
            let seg_start = split.e_end - suffix_len;
            if raw.get(seg_start..split.e_end) == Some(suffix_raw) {
                let mut out = String::with_capacity(raw.len() - suffix_len);
                out.push_str(&raw[..seg_start]);
                out.push_str(&raw[split.e_end..]);
                return out;
            }
        }
    }
    // Fallback: remove the first occurrence.
    if let Some(pos) = raw.find(suffix_raw) {
        let mut out = String::with_capacity(raw.len() - suffix_raw.len());
        out.push_str(&raw[..pos]);
        out.push_str(&raw[pos + suffix_raw.len()..]);
        out
    } else {
        raw.to_string()
    }
}

/// Insert suffix bytes at the end of `E` (before any repair `!` or second
/// `=`), preserving the target's own repair and close part.
fn insert_suffix_text(raw: &str, suffix_raw: &str) -> String {
    if suffix_raw.is_empty() {
        return raw.to_string();
    }
    if let Some(split) = split_past_token(raw) {
        let mut out = String::with_capacity(raw.len() + suffix_raw.len());
        out.push_str(&raw[..split.e_end]);
        out.push_str(suffix_raw);
        out.push_str(&raw[split.e_end..]);
        return out;
    }
    format!("{raw}{suffix_raw}")
}
