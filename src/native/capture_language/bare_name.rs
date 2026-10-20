//! Shared bare-name predicate for `=name` named starts.
//!
//! Every position (lexer, link suffix, completion, hints) calls these
//! helpers instead of re-deriving when a word after `=` is a Pomodoro
//! name. A name can be bare when it starts with an ASCII letter and is
//! not close-shaped (`x`, `x2`, `x!`, `*`, `!`). Digit-led names and
//! close lookalikes keep the `#` spelling (`=#2fa`, `=#x`).

use super::close_selection::close_after_eq_parts;
use super::tokens::is_pomodoro_selector_component;

/// How a named start spelled the name after `=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameSpelling {
    /// `=#name` / `=3#name`: the `#` is present.
    Hash,
    /// `=name`: no `#`, only when [`can_spell_bare`] holds.
    Bare,
}

/// Byte length of a `#name` / bare-name part: ends at whitespace, `~`, or
/// `@`, or at the end of `text`.
pub(crate) fn start_name_extent(text: &str) -> usize {
    text.find(|character: char| {
        character.is_ascii_whitespace() || character == '~' || character == '@'
    })
    .unwrap_or(text.len())
}

/// `Some` when the text right after a single `=` begins a bare name.
///
/// That means its first byte is an ASCII letter,
/// [`close_after_eq_parts`] does not claim it, and it does not start with
/// `x#`/`X#`. The returned length is the name's extent.
pub(crate) fn bare_start_name_len(after_eq: &str) -> Option<usize> {
    if !after_eq
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return None;
    }
    if close_after_eq_parts(after_eq).is_some() {
        return None;
    }
    if after_eq.starts_with("x#") || after_eq.starts_with("X#") {
        return None;
    }
    let len = start_name_extent(after_eq);
    (len > 0).then_some(len)
}

/// Whether `name` can be written after `=` without `#`.
pub(crate) fn can_spell_bare(name: &str) -> bool {
    is_pomodoro_selector_component(name)
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
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
        NameSpelling::Bare => format!("={name}"),
        NameSpelling::Hash => format!("={suffix}#{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_name_len_matches_letter_led_non_close_shapes() {
        assert_eq!(bare_start_name_len("bugs"), Some(4));
        assert_eq!(bare_start_name_len("bugs@3"), Some(4));
        assert_eq!(bare_start_name_len("bugs~2"), Some(4));
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
        assert!(can_spell_bare("deep-work"));
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
        assert_eq!(start_name_spelling("x"), "#x");
        assert_eq!(named_start_token("bugs"), "=bugs");
        assert_eq!(named_start_token(""), "=#");
        assert_eq!(named_start_token("2fa"), "=#2fa");
        assert_eq!(echo_named_start(NameSpelling::Bare, "", "bugs"), "=bugs");
        assert_eq!(
            echo_named_start(NameSpelling::Hash, "3", "bugs"),
            "=3#bugs"
        );
    }
}
