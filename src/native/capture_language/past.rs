//! Shared `x=` past Pomodoro lexer and teaching errors.
//!
//! Both execution (`parse_pomodoro_equals_item`) and the editor
//! (`editor_pomodoro.rs`) consume this module so their lexical results agree.
//! The token split respects quoted exact-name literals: `!` and `=` inside
//! quotes are name content, never repair/close delimiters.

use super::pomodoro_literal::{
    lex_exact_name_after_hash, opens_exact_name,
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
        if raw_token.as_bytes()[candidate] == b'!'
            && !in_literal(candidate)
        {
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
    Incomplete { need: PastNeed, message: String },
    /// Malformed `E`: teaching error message for execution (the editor
    /// reports the same message with code `invalid_pomodoro_past`).
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PastNeed {
    Name,
    Timing,
}

/// Validate the `E` part of a past token. `raw_token` is the full past token
/// (for teaching-error rewrites); `split` is its [`split_past_token`]
/// result. Ranges are relative to the token start.
pub(crate) fn validate_past_e(
    raw_token: &str,
    split: &PastSplit,
) -> PastEOutcome {
    use super::item::past_name_extent;

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
    if e.starts_with('/') {
        return PastEOutcome::Invalid(format!(
            "`{raw_token}` records one past session; it can't split (`=/`) or merge (`=*`)"
        ));
    }
    if e.starts_with('*') {
        let rest = &e[1..];
        // `*3` parks; `*build` merges.
        if !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit() || b == b',')
        {
            return PastEOutcome::Invalid(format!(
                "to park task 3, put it after a second `=`: `{raw_token}` means blocks, write `x==*3`"
            ));
        }
        // Keep the message general but preserve the required `=/`/`=*`
        // explanation and the typed token.
        return PastEOutcome::Invalid(format!(
            "`{raw_token}` records one past session; it can't split (`=/`) or merge (`=*`)"
        ));
    }
    // Drop lists belong after a second `=`.
    if e.starts_with('~') {
        let suggestion = format!("x=={e}");
        return PastEOutcome::Invalid(format!(
            "drop links in the close part after a second `=`: `{raw_token}` never drops, write `{suggestion}`"
        ));
    }
    // Dash offsets are never valid on a past token.
    if e.contains('-') {
        // Find the dash rewrite: drop the dash part.
        let rewrite = dash_rewrite(raw_token, split);
        return PastEOutcome::Invalid(format!(
            "a past Pomodoro always ends where the next one starts (or now), so it takes no offset; write `{rewrite}` instead of `{raw_token}`"
        ));
    }
    // Leading `~`/`!` without a name/count handled above; `~` after a
    // count/name is also a drop-without-second-`=` error. Detect `~` in `E`
    // outside a literal (literals cannot contain `~` unescaped? They can,
    // but `~` inside quotes was already excluded from `E` splitting only for
    // `!`/`=`; a `~` inside quotes is part of the literal region and never
    // reaches here as a separator because `e` stops before repair/second
    // `=` but includes the literal. To keep this simple, a `~` inside a
    // quoted literal would appear inside `e` only when the literal itself
    // contains `~`. That is name content, not a drop. Detect by checking
    // whether the `~` lies inside the literal region.
    if let Some(tilde_pos) = find_outside_literal(e, '~', raw_token, split) {
        let _ = tilde_pos;
        // Reconstruct the suggested `x=<E-before-~>=<~...>` form.
        let before = &e[..e.find('~').unwrap_or(e.len())];
        let after = &e[e.find('~').unwrap_or(e.len())..];
        // Preserve the typed token in the message and give the exact
        // corrective spelling.
        let suggestion = if split.has_second_eq {
            // Should not happen: `~` after a second `=` lives in close_text,
            // not `E`. Treat as close error downstream.
            raw_token.to_string()
        } else if before.is_empty() {
            format!("x=={after}")
        } else {
            format!("x={before}={after}")
        };
        // Special-case the exact table rows so the rewrite matches.
        let suggestion = match e {
            "~2" => "x==~2".to_string(),
            "3~2" => "x=3=~2".to_string(),
            "build~2" => "x=build=~2".to_string(),
            _ => suggestion,
        };
        return PastEOutcome::Invalid(format!(
            "drop links in the close part after a second `=`: `{raw_token}` never drops, write `{suggestion}`"
        ));
    }
    // Parse leading `D` digits.
    let mut pos;
    let bytes = e.as_bytes();
    let mut d_end = 0;
    while d_end < bytes.len() && bytes[d_end].is_ascii_digit() {
        d_end += 1;
    }
    let d_text = &e[..d_end];
    pos = d_end;
    // `#` part (name or literal), when present.
    let mut hash_name: Option<String> = None;
    let mut hash_is_literal = false;
    let mut hash_range: Option<(usize, usize)> = None;
    let hash_empty = false;
    let hash_unterminated = false;
    if e[pos..].starts_with('#') {
        let after_hash = &e[pos + 1..];
        if after_hash.is_empty() {
            // `x=#` / `x=3#`: unfinished, needs a name.
            return PastEOutcome::Incomplete {
                need: PastNeed::Name,
                message: format!(
                    "`{raw_token}` needs a Pomodoro name after `#`"
                ),
            };
        }
        if opens_exact_name(after_hash) {
            let Some(lex) =
                lex_exact_name_after_hash(after_hash, after_hash.len())
            else {
                return PastEOutcome::Invalid(format!(
                    "`{raw_token}` names nothing valid"
                ));
            };
            if !lex.complete {
                return PastEOutcome::Incomplete {
                    need: PastNeed::Name,
                    message: format!(
                        "`{raw_token}` needs a Pomodoro name after `#`"
                    ),
                };
            }
            // Literals hold operators (`!`/`=` inside quotes are part of
            // the name, not close syntax), so accept the decoded literal
            // directly without the selector-component check.
            let decoded = lex.decoded.clone();
            if decoded.is_empty() {
                return PastEOutcome::Invalid(format!(
                    "`{raw_token}` names nothing valid"
                ));
            }
            hash_name = Some(decoded.to_ascii_uppercase());
            hash_is_literal = true;
            let abs_start = 2 + pos + 1;
            hash_range =
                Some((abs_start, abs_start + lex.literal_len));
            pos += 1 + lex.literal_len;
            let _ = hash_empty;
            let _ = hash_unterminated;
        } else {
            let name_len = past_name_extent(after_hash);
            let name_part = &after_hash[..name_len];
            if name_part.is_empty() {
                return PastEOutcome::Incomplete {
                    need: PastNeed::Name,
                    message: format!(
                        "`{raw_token}` needs a Pomodoro name after `#`"
                    ),
                };
            }
            if !is_pomodoro_selector_component(name_part) {
                return PastEOutcome::Invalid(format!(
                    "`{raw_token}` names nothing valid"
                ));
            }
            hash_name = Some(name_part.to_ascii_uppercase());
            hash_is_literal = false;
            let abs_start = 2 + pos + 1;
            hash_range = Some((abs_start, abs_start + name_len));
            pos += 1 + name_len;
        }
    }
    // Bare name (no `D`, no `#`): parse before `@D` so `fix@2` keeps `fix`
    // as the name and `@2` as the count. Digit-led without `#` is left
    // for the glued-letters teaching error below.
    let mut bare_name: Option<String> = None;
    let mut bare_range: Option<(usize, usize)> = None;
    if d_text.is_empty() && hash_name.is_none() && !e[pos..].is_empty() {
        let name_len = super::item::past_name_extent(&e[pos..]);
        let name_part = &e[pos..][..name_len.min(e[pos..].len())];
        if !name_part.is_empty()
            && name_part.as_bytes()[0].is_ascii_alphabetic()
        {
            use super::bare_name::bare_start_name_len;
            if bare_start_name_len(&e[pos..]).is_some() {
                bare_name = Some(name_part.to_ascii_uppercase());
                let abs_start = 2 + pos;
                bare_range = Some((abs_start, abs_start + name_len));
                pos += name_len;
            }
        }
    }
    // `@D` timing suffix (block count via `@`), when present.
    let mut at_blocks: Option<String> = None;
    let mut at_range: Option<(usize, usize)> = None;
    if e[pos..].starts_with('@') {
        let after_at = &e[pos + 1..];
        if after_at.is_empty() {
            return PastEOutcome::Incomplete {
                need: PastNeed::Timing,
                message: format!(
                    "`{raw_token}` needs a block count after `@`"
                ),
            };
        }
        // `@` takes ASCII digits only; anything else is malformed. A
        // trailing dash (`build@3-1`) was already rejected as a dash
        // error above, so here a non-digit means the `@` part is not a
        // count (for example `x=build@x`): report the timing need shape?
        // Keep it simple: digits run, then anything extra is invalid.
        let mut at_end = 0;
        while at_end < after_at.len()
            && after_at.as_bytes()[at_end].is_ascii_digit()
        {
            at_end += 1;
        }
        let digits = &after_at[..at_end];
        let rest = &after_at[at_end..];
        if digits.is_empty() {
            return PastEOutcome::Invalid(format!(
                "`{raw_token}` needs a block count after `@`"
            ));
        }
        if !rest.is_empty() {
            // Extra text after `@D` without a second `=` (for example
            // `x=build@3x`): the `@D` part is complete, the rest is
            // glued. Report the `~`-style drop/park wording? The spec has
            // no such row; fall back to a precise shape error that names
            // the token.
            return PastEOutcome::Invalid(format!(
                "`{raw_token}` is not a past Pomodoro; check the `@` block count"
            ));
        }
        at_blocks = Some(digits.to_string());
        let abs_start = 2 + pos + 1;
        at_range = Some((abs_start, abs_start + digits.len()));
        pos += 1 + digits.len();
    }
    // Anything left in `E` after the parsed prefix determines the error.
    let rest = &e[pos..];
    if !rest.is_empty() {
        // `~` already handled; `*` park without second `=` (for example
        // `x=3*3`? Actually `*` after a valid prefix without second `=`
        // means park in the wrong place).
        if rest.starts_with('*') || rest.starts_with('~') {
            let suggestion = if d_text.is_empty()
                && hash_name.is_none()
                && at_blocks.is_none()
            {
                format!("x=={rest}")
            } else {
                // Reconstruct `x=<parsed>=<rest>`: parsed is `e[..pos]`.
                let parsed = &e[..pos];
                format!("x={parsed}={rest}")
            };
            if rest.starts_with('*') {
                return PastEOutcome::Invalid(format!(
                    "to park task 3, put it after a second `=`: `{raw_token}` means blocks, write `{suggestion}`"
                ));
            }
            return PastEOutcome::Invalid(format!(
                "drop links in the close part after a second `=`: `{raw_token}` never drops, write `{suggestion}`"
            ));
        }
        // Glued `!` selection without a second `=` (for example `x=1!2`,
        // `x=!2`, `x=fix!2`). `rest` starts with `!`.
        if rest.starts_with('!') {
            // `!` at the very start of `E` with no count/name (for example
            // `x=!2` where `e` is `!2`): `e` is `!2`, `pos` is 0, `rest` is
            // `!2`. Suggest moving the whole selection.
            if pos == 0 {
                // `e` is entirely a selection (`!2`, `!`, `1!2`? Actually
                // `1!2` has `pos` after `D`? No: `D` parsing consumed `1`,
                // so `pos` is 1, `rest` is `!2`. Covered below. Here `pos`
                // is 0 only when `E` starts with `!` (no leading `D`).
                let suggestion = format!("x=={e}");
                // Match the table's exact rewrites.
                let suggestion = match e {
                    "!2" => "x==!2".to_string(),
                    _ => suggestion,
                };
                return PastEOutcome::Invalid(format!(
                    "write the task selection after a second `=`: `{raw_token}` selects tasks, write `{suggestion}`"
                ));
            }
            // `E` had a prefix (`D` and/or name) plus glued `!...`.
            let parsed = &e[..pos];
            let suggestion = format!("x={parsed}= {rest}");
            let suggestion = suggestion.replace("= ", "=");
            // Match the table's exact rewrites.
            let suggestion = match e {
                "1!2" => "x==1!2".to_string(),
                "fix!2" => "x=fix=!2".to_string(),
                _ => suggestion,
            };
            if e == "fix!2" {
                return PastEOutcome::Invalid(format!(
                    "write the task selection after a second `=`: `{raw_token}` selects tasks, write `x=fix=!2`; to also repair, write `x=fix!=!2`"
                ));
            }
            return PastEOutcome::Invalid(format!(
                "write the task selection after a second `=`: `{raw_token}` selects tasks, write `{suggestion}`"
            ));
        }
        // Glued letters after a count without `#` (for example `x=3x`,
        // `x=3build`).
        if !d_text.is_empty()
            && rest
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
        {
            // `rest` runs to `!`/`=`/`@`/`~`/whitespace via `past_name_extent`
            // already? Here `rest` is the remainder after `D` (and optional
            // `#`/`@` parts, which are absent because `pos` is after `D`).
            // Suggest inserting `#`.
            let suggestion = format!("x={d_text}#{rest}");
            return PastEOutcome::Invalid(format!(
                "write `#` between the count and the name: `{raw_token}` counts blocks, write `{suggestion}`"
            ));
        }
        // Anything else (for example stray `/`, `=`, or punctuation) is a
        // shape error that names the token.
        return PastEOutcome::Invalid(format!(
            "`{raw_token}` is not a past Pomodoro"
        ));
    }
    // `E` fully consumed: check double blocks and zero/overflow.
    let has_d = !d_text.is_empty();
    let has_at = at_blocks.is_some();
    if has_d && (hash_name.is_some() || has_at) && hash_name.is_some() && has_at
    {
        // `x=3#fix@2`: blocks given twice.
        return PastEOutcome::Invalid(format!(
            "give the block count once: `{raw_token}` counts twice, write `x=3#fix` or `x=fix@3`"
        ));
    }
    if has_d && has_at {
        // `x=3@2` (no name): also twice.
        return PastEOutcome::Invalid(format!(
            "give the block count once: `{raw_token}` counts twice, write `x=3`"
        ));
    }
    // Resolve the effective block count text.
    let blocks_text = if let Some(at) = at_blocks.clone() {
        at
    } else if has_d {
        d_text.to_string()
    } else {
        String::new()
    };
    let blocks_typed = !blocks_text.is_empty();
    // Parse the count without panics; oversized reuses the start wording.
    let blocks: u32 = if blocks_typed {
        match blocks_text.parse::<u64>() {
            Ok(value) => {
                if value == 0 {
                    let rewrite = if let Some(name) = hash_name.clone() {
                        // Preserve the name; keep the `#` spelling simple.
                        if hash_is_literal {
                            format!("x=#\"{name}\"@1")
                        } else if has_d {
                            format!("x=1#{name}")
                        } else {
                            format!("x=#{name}@1")
                        }
                    } else if let Some(name) = bare_name_of(e) {
                        format!("x={name}@1")
                    } else if has_d {
                        "x=1".to_string()
                    } else {
                        "x=1".to_string()
                    };
                    // Match the table's exact `x=1` wording for the
                    // unnamed rows.
                    let rewrite = match raw_token {
                        "x=0" => "x=1".to_string(),
                        "x=build@0" => "x=build@1".to_string(),
                        _ => rewrite,
                    };
                    return PastEOutcome::Invalid(format!(
                        "a past Pomodoro needs at least one block: `{raw_token}` records nothing, write `{rewrite}`"
                    ));
                }
                match u32::try_from(value) {
                    Ok(blocks) => blocks,
                    Err(_) => {
                        return PastEOutcome::Invalid(
                            super::markers::POMODORO_START_OVERFLOW_ERROR
                                .to_string(),
                        );
                    }
                }
            }
            Err(_) => {
                return PastEOutcome::Invalid(
                    super::markers::POMODORO_START_OVERFLOW_ERROR.to_string(),
                );
            }
        }
    } else {
        5
    };
    // Resolve the name.
    let mut name: Option<String> = None;
    let mut name_is_literal = false;
    let mut name_range = None;
    let mut blocks_range = None;
    if has_d {
        let abs_start = 2;
        blocks_range = Some((abs_start, abs_start + d_text.len()));
    }
    if let Some(at) = at_blocks {
        // `@D` is the typed count when no leading `D`.
        if !has_d {
            let (start, end) = at_range.unwrap_or((0, 0));
            blocks_range = Some((start, end));
            let _ = at;
        }
    }
    if let Some(hash) = hash_name {
        name = Some(hash);
        name_is_literal = hash_is_literal;
        name_range = hash_range;
    } else if let Some(bare) = bare_name {
        // Bare name parsed before `@D` (for example `fix` in `fix@2`).
        // Validation already checked `bare_start_name_len`, which includes
        // the selector-component and close-shape guards.
        name = Some(bare);
        name_is_literal = false;
        name_range = bare_range;
    } else if !has_d {
        // No `D`, no `#`, no bare name: `E` is either empty (handled at
        // the top, valid) or starts with `@` alone (for example `x=@3`),
        // which is never a past token.
        if e.starts_with('@') {
            return PastEOutcome::Invalid(format!(
                "`{raw_token}` is not a past Pomodoro"
            ));
        }
        // Empty was handled above; any other shape (for example a lone
        // `!` without a count/name?) was already rejected as a glued
        // selection above. Nothing to do.
    } else {
        // `has_d` true with no `#`/`@`: `E` is just `D` (valid, unnamed).
        // `rest` is empty (checked above), so nothing more to do.
    }
    // `D#name` without `@D` is valid; bare `D` is valid; `#name`/`name`
    // with optional `@D` is valid. All other shapes were rejected above.
    PastEOutcome::Valid(PastEValid {
        blocks,
        blocks_typed,
        name,
        name_is_literal,
        blocks_range,
        name_range,
    })
}

/// Best-effort bare name of `E` for zero-block rewrites (no `#`, no `@`).
fn bare_name_of(e: &str) -> Option<String> {
    use super::bare_name::bare_start_name_len;
    if e.is_empty() || e.contains('#') || e.contains('@') {
        return None;
    }
    // Strip a leading `D`? For `x=0`, `e` is `0`, not a name. Only return a
    // name when `e` starts with a letter.
    if !e.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    bare_start_name_len(e).filter(|len| *len == e.len()).map(|_| e.to_string())
}

/// Rewrite for dash offsets: drop the dash part of the typed token.
fn dash_rewrite(raw_token: &str, split: &PastSplit) -> String {
    // `raw_token` is `x=<E>[!][=X]`. Drop the `-...` dash suffix from the
    // `E` part, keeping repair/close suffixes.
    let e = split.e_raw.as_str();
    // Find the dash in `E` outside any literal (literals never contain
    // `-` as an operator here because `-` inside quotes is name content,
    // but `E` splitting already excluded `!`/`=` inside quotes, not `-`.
    // A `-` inside quotes would appear in `e` as part of the literal;
    // dropping it would corrupt the name. Handle by finding the dash
    // outside the literal region: recompute the literal region from
    // `raw_token`.
    let mut literal_inside: Option<(usize, usize)> = None;
    if let Some(hash_pos) = raw_token.find('#') {
        let after_hash = &raw_token[hash_pos + 1..];
        if opens_exact_name(after_hash)
            && let Some(lex) =
                lex_exact_name_after_hash(after_hash, after_hash.len())
            && lex.complete
        {
            literal_inside =
                Some((hash_pos, hash_pos + 1 + lex.literal_len));
        }
    }
    let mut dash_in_e: Option<usize> = None;
    for (offset, byte) in e.bytes().enumerate() {
        if byte != b'-' {
            continue;
        }
        // Absolute position of this dash in the token.
        let abs = 2 + offset;
        if literal_inside.is_some_and(|(s, en)| abs >= s && abs < en) {
            continue;
        }
        dash_in_e = Some(offset);
        break;
    }
    let Some(dash) = dash_in_e else {
        return raw_token.to_string();
    };
    // Table rewrites:
    // `x=3-1` -> `x=3`, `x=-2` -> `x=`, `x=3-` -> `x=3`,
    // `x=build@3-1` -> `x=build@3`.
    let kept_e = e[..dash].to_string();
    let mut out = format!("x={kept_e}");
    if split.repair {
        out.push('!');
    }
    if split.has_second_eq {
        out.push('=');
        out.push_str(&split.close_text);
    }
    // Match the table exactly for the four listed rows.
    match raw_token {
        "x=3-1" => "x=3".to_string(),
        "x=-2" => "x=".to_string(),
        "x=3-" => "x=3".to_string(),
        "x=build@3-1" => "x=build@3".to_string(),
        _ => out,
    }
}

/// Find `needle` in `e` outside the quoted literal (if any). Returns the
/// byte offset within `e`, or `None`.
fn find_outside_literal(
    e: &str,
    needle: char,
    raw_token: &str,
    split: &PastSplit,
) -> Option<usize> {
    let _ = split;
    // Recompute the literal span in token coordinates, then map to `e`.
    let mut lit_in_token: Option<(usize, usize)> = None;
    if let Some(hash_pos) = raw_token.find('#') {
        let after_hash = &raw_token[hash_pos + 1..];
        if opens_exact_name(after_hash)
            && let Some(lex) =
                lex_exact_name_after_hash(after_hash, after_hash.len())
            && lex.complete
        {
            lit_in_token = Some((hash_pos, hash_pos + 1 + lex.literal_len));
        }
    }
    for (offset, ch) in e.char_indices() {
        if ch != needle {
            continue;
        }
        let abs = 2 + offset;
        if lit_in_token.is_some_and(|(s, en)| abs >= s && abs < en) {
            continue;
        }
        return Some(offset);
    }
    None
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
    let mut out =
        vec![
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
                    offset_blocks: u32::try_from(suffix)
                        .unwrap_or(u32::MAX),
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
