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

/// Validated `E` part: block count and optional typed name.
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
                if !lexed.is_literal {
                    let typed = &e[lexed.start..lexed.end];
                    if can_spell_bare(typed) {
                        let rewrite =
                            format!("x={d_text}{typed}{}", past_suffix(split));
                        return PastEOutcome::Invalid(
                            retired_start_spelling_error(
                                raw_token,
                                "`#` is only for names a bare spelling cannot express",
                                &rewrite,
                            ),
                        );
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
        let base = &e[..pos];
        // A `~`/`*` tail after the retired `@D` moves to the close part.
        let tail = &after_at[digits..];
        let moved = (!tail.is_empty()).then_some(format!("={tail}"));
        let glued = if d_end > 0 {
            // A leading count already sets the blocks: drop the `@D`.
            base.to_string()
        } else if let Some(lexed) = name.as_ref().filter(|n| n.is_literal) {
            // Literals take timing before the `#`.
            let raw = &e[lexed.start..lexed.end];
            format!("{at_count}#{raw}")
        } else {
            format!("{base}{at_count}")
        };
        let rewrite = format!(
            "x={glued}{}{}",
            moved.unwrap_or_default(),
            past_suffix(split)
        );
        return PastEOutcome::Invalid(retired_start_spelling_error(
            raw_token,
            "`@` no longer separates a name from its timing",
            &rewrite,
        ));
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
        let kept = after_dash.trim_start_matches(|c: char| c.is_ascii_digit());
        let rewrite = format!("x={parsed}{kept}{suffix}");
        return format!(
            "a past Pomodoro always ends where the next one starts (or now), so it takes no offset; write `{rewrite}` instead of `{raw_token}`"
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
    if rest.starts_with('~') {
        let rewrite = past_close_rewrite(parsed, rest, split);
        return format!(
            "drop links in the close part after a second `=`: `{raw_token}` never drops, write `{rewrite}`"
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

/// Compute stack positions for a sequence of optional block counts.
/// `blocks[i]` is `Some(blocks)` for a valid Past item, `None` for every
/// other item (including invalid or unfinished Past, which break runs).
/// Returns one [`PastStackPos`] per input position (zeros for `None`).
/// A stack is a maximal run of consecutive `Some`; each member gets its
/// 1-based index, the run size, and the sum of later members' blocks (with
/// a wide `u64` sum saturated to `u32` so large counts cannot overflow
/// text-only parsing).
pub(crate) fn compute_past_stacks(
    blocks: &[Option<u32>],
) -> Vec<super::PastStackPos> {
    use super::PastStackPos;
    let mut out = vec![
        PastStackPos {
            index: 0,
            size: 0,
            offset_blocks: 0,
        };
        blocks.len()
    ];
    let mut run_start: Option<usize> = None;
    for (i, b) in blocks.iter().enumerate() {
        let is_past = b.is_some();
        let run_end = if is_past {
            if i + 1 == blocks.len() || blocks[i + 1].is_none() {
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
                let blk = blocks[k].unwrap_or(0) as u64;
                out[k] = PastStackPos {
                    index: (k - start + 1) as u32,
                    size,
                    offset_blocks: u32::try_from(suffix).unwrap_or(u32::MAX),
                };
                suffix = suffix.saturating_add(blk);
            }
            run_start = None;
        }
        if !is_past {
            run_start = None;
        }
    }
    out
}
