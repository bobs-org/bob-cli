//! Shared bare-name predicate for `=name` named starts.
//!
//! Every position (lexer, link suffix, completion, hints) calls these
//! helpers instead of re-deriving when a word after `=` is a Pomodoro
//! name. A bare name starts with an ASCII letter, contains no dash, and
//! does not end in an ASCII digit or `+`. Close-shaped names and names
//! that end in `+` keep the `#` spelling.

use super::close_selection::close_after_eq_parts;
use super::tokens::{
    is_pomodoro_selector_byte, is_pomodoro_selector_component,
};

/// How a named start spelled the name after `=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameSpelling {
    /// `=#name` / `=3#name`: the `#` is present.
    Hash,
    /// `=name`: no `#`, only when [`can_spell_bare`] holds.
    Bare,
}

/// Byte length of a `#name` part: ends at whitespace, `~`, `=`, `!`, or
/// `@`, or at the end of `text`.
pub(crate) fn start_name_extent(text: &str) -> usize {
    text.find(|character: char| {
        character.is_ascii_whitespace()
            || matches!(character, '~' | '=' | '!' | '@')
    })
    .unwrap_or(text.len())
}

/// `Some` when the text right after a single `=` begins a bare name.
///
/// That means its first byte is an ASCII letter. The returned length omits
/// a maximal trailing run of ASCII digits and `+`, which belongs to the
/// trailing timing (`fix2+` → `fix`, `bob+decks2+3` → `bob+decks`). A name
/// that ends in a letter keeps interior `+` bytes (`bob+2fa`, `bob+decks`).
pub(crate) fn bare_start_name_len(after_eq: &str) -> Option<usize> {
    if !after_eq
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return None;
    }
    if after_eq.starts_with("x#") || after_eq.starts_with("X#") {
        return None;
    }
    let len = after_eq
        .bytes()
        .take_while(|byte| is_pomodoro_selector_byte(*byte) && *byte != b'-')
        .count();
    let candidate = after_eq.get(..len)?;
    let mut name_len = candidate.len();
    while name_len > 0 {
        let last = candidate.as_bytes()[name_len - 1];
        if last.is_ascii_digit() || last == b'+' {
            name_len -= 1;
        } else {
            break;
        }
    }
    if name_len == 0 {
        return None;
    }
    let name = &candidate[..name_len];
    if close_after_eq_parts(name).is_some() {
        return None;
    }
    Some(name_len)
}

/// Whether `name` can be written after `=` without `#`.
pub(crate) fn can_spell_bare(name: &str) -> bool {
    is_pomodoro_selector_component(name)
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && !name.contains('-')
        && !name.as_bytes().last().is_some_and(u8::is_ascii_digit)
        && !name.ends_with('+')
        && close_after_eq_parts(name).is_none()
}

/// The text after `=` for a composed named start: `name`, `#name`, or
/// `#` when `name` is empty.
pub(crate) fn start_name_spelling(name: &str) -> String {
    if name.is_empty() {
        return "#".to_string();
    }
    if can_spell_bare(name) {
        name.to_string()
    } else {
        format!("#{name}")
    }
}

/// A composed named-start token: `=name`, `=#name`, or `=#` when `name`
/// is empty.
pub(crate) fn named_start_token(name: &str) -> String {
    format!("={}", start_name_spelling(name))
}

/// Echo a typed named start, keeping the user's `#` or bare spelling.
pub(crate) fn echo_named_start(
    spelling: NameSpelling,
    suffix: &str,
    name: &str,
) -> String {
    match spelling {
        NameSpelling::Bare => format!("={suffix}{name}"),
        NameSpelling::Hash => format!("={suffix}#{name}"),
    }
}

/// Compose a valid named-start token from at most one timing and a name.
/// Non-bare names take timing before `#`; bare names keep it on either side.
pub(crate) fn glued_named_start(
    leading: &str,
    name: &str,
    trailing: &str,
) -> String {
    if can_spell_bare(name) {
        if !leading.is_empty() {
            format!("={leading}{name}")
        } else {
            format!("={name}{trailing}")
        }
    } else {
        let timing = if leading.is_empty() {
            trailing
        } else {
            leading
        };
        format!("={timing}#{name}")
    }
}

/// Trailing glued `<X>` inside a bare-name replacement slice (`de3`,
/// `fix10-3`, `fix2+`, `de3+1`). Hash-delimited slices keep their digits
/// as name bytes. A retired `@` after the name is not glued timing.
pub(crate) fn trailing_glued_timing(slice: &str) -> &str {
    if slice.starts_with('#') {
        return "";
    }
    let Some(name_len) = bare_start_name_len(slice) else {
        return "";
    };
    if name_len >= slice.len() {
        return "";
    }
    let rest = &slice[name_len..];
    if rest.starts_with('@') {
        return "";
    }
    let bytes = rest.as_bytes();
    let mut len = 0;
    while len < bytes.len() && bytes[len].is_ascii_digit() {
        len += 1;
    }
    if matches!(bytes.get(len), Some(b'-' | b'+')) {
        loop {
            len += 1;
            while len < bytes.len() && bytes[len].is_ascii_digit() {
                len += 1;
            }
            if !matches!(bytes.get(len), Some(b'-' | b'+')) {
                break;
            }
        }
    }
    &rest[..len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_name_len_matches_letter_led_non_close_shapes() {
        assert_eq!(bare_start_name_len("bugs"), Some(4));
        assert_eq!(bare_start_name_len("bugs@3"), Some(4));
        assert_eq!(bare_start_name_len("bugs~2"), Some(4));
        assert_eq!(bare_start_name_len("fix3"), Some(3));
        assert_eq!(bare_start_name_len("fix10-3"), Some(3));
        assert_eq!(bare_start_name_len("b2b"), Some(3));
        assert_eq!(bare_start_name_len("xmas3"), Some(4));
        assert_eq!(bare_start_name_len("bob+decks2"), Some(9));
        assert_eq!(bare_start_name_len("fix2+"), Some(3));
        assert_eq!(bare_start_name_len("bob+decks2+3"), Some(9));
        assert_eq!(bare_start_name_len("bob+2fa"), Some(7));
        assert_eq!(bare_start_name_len("xx+"), Some(2));
        assert_eq!(bare_start_name_len("b2b+2"), Some(3));
        assert_eq!(bare_start_name_len("c++"), Some(1));
        assert_eq!(bare_start_name_len("xx"), Some(2));
        assert_eq!(bare_start_name_len("xmas"), Some(4));
        assert_eq!(bare_start_name_len("Bugs"), Some(4));
        assert_eq!(bare_start_name_len("x"), None);
        assert_eq!(bare_start_name_len("x2"), None);
        assert_eq!(bare_start_name_len("x#bugs"), None);
        assert_eq!(bare_start_name_len("X#bugs"), None);
        assert_eq!(bare_start_name_len("*"), None);
        assert_eq!(bare_start_name_len("!"), None);
        assert_eq!(bare_start_name_len("3bugs"), None);
        assert_eq!(bare_start_name_len("2fa"), None);
        assert_eq!(bare_start_name_len(""), None);
        assert_eq!(bare_start_name_len("="), None);
    }

    #[test]
    fn can_spell_bare_rejects_digit_led_and_close_shapes() {
        assert!(can_spell_bare("bugs"));
        assert!(can_spell_bare("xx"));
        assert!(can_spell_bare("xmas"));
        assert!(!can_spell_bare("deep-work"));
        assert!(!can_spell_bare("fix3"));
        assert!(can_spell_bare("b2b"));
        assert!(can_spell_bare("bob+decks"));
        assert!(!can_spell_bare("c++"));
        assert!(!can_spell_bare("fix+"));
        assert!(!can_spell_bare("bob+decks2"));
        assert!(!can_spell_bare("2fa"));
        assert!(!can_spell_bare("x"));
        assert!(!can_spell_bare("x2"));
        assert!(!can_spell_bare("x2!1"));
        assert!(!can_spell_bare(""));
        assert!(!can_spell_bare("b!"));
    }

    #[test]
    fn composed_tokens_keep_hash_when_the_name_cannot_be_bare() {
        assert_eq!(start_name_spelling("bugs"), "bugs");
        assert_eq!(start_name_spelling(""), "#");
        assert_eq!(start_name_spelling("2fa"), "#2fa");
        assert_eq!(start_name_spelling("deep-work"), "#deep-work");
        assert_eq!(start_name_spelling("fix3"), "#fix3");
        assert_eq!(start_name_spelling("x"), "#x");
        assert_eq!(named_start_token("bugs"), "=bugs");
        assert_eq!(named_start_token(""), "=#");
        assert_eq!(named_start_token("2fa"), "=#2fa");
        assert_eq!(echo_named_start(NameSpelling::Bare, "", "bugs"), "=bugs");
        assert_eq!(
            echo_named_start(NameSpelling::Hash, "3", "deep-work"),
            "=3#deep-work"
        );
        assert_eq!(glued_named_start("", "fix", "3"), "=fix3");
        assert_eq!(glued_named_start("3", "fix", ""), "=3fix");
        assert_eq!(glued_named_start("", "deep-work", "3"), "=3#deep-work");
        assert_eq!(glued_named_start("3", "deep-work", ""), "=3#deep-work");
        assert_eq!(glued_named_start("", "fix", "2+"), "=fix2+");
        assert_eq!(glued_named_start("", "deep-work", "2+1"), "=2+1#deep-work");
        assert_eq!(glued_named_start("", "c++", ""), "=#c++");
        assert_eq!(trailing_glued_timing("de3"), "3");
        assert_eq!(trailing_glued_timing("fix10-3"), "10-3");
        assert_eq!(trailing_glued_timing("fix2+"), "2+");
        assert_eq!(trailing_glued_timing("de3+1"), "3+1");
        assert_eq!(trailing_glued_timing("fix2+~1"), "2+");
        assert_eq!(trailing_glued_timing("#de3"), "");
        assert_eq!(trailing_glued_timing("de@3"), "");
        assert_eq!(trailing_glued_timing("de3~2"), "3");
        assert_eq!(trailing_glued_timing("de"), "");
    }
}
