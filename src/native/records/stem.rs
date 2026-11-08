use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::native::capture_block_ids::is_valid_block_id;

/// Normalize a record title into a filename stem. `None` means the title has
/// no alphanumeric characters and needs an explicit stem.
pub(crate) fn normalize_stem(title: &str) -> Option<String> {
    let mut folded = String::new();
    for character in title.nfkd() {
        if is_combining_mark(character) {
            continue;
        }
        if character == '&' {
            folded.push('_');
            folded.push_str("and");
            folded.push('_');
        } else if character == '\'' || character == '’' {
            continue;
        } else if character.is_alphanumeric() {
            for lower in character.to_lowercase() {
                folded.push(lower);
            }
        } else {
            folded.push('_');
        }
    }

    let mut stem = String::with_capacity(folded.len());
    let mut previous_separator = true;
    for character in folded.chars() {
        if character == '_' {
            if !previous_separator {
                stem.push('_');
            }
            previous_separator = true;
        } else {
            stem.push(character);
            previous_separator = false;
        }
    }
    if stem.ends_with('_') {
        stem.pop();
    }
    (!stem.is_empty()).then_some(stem)
}

/// Explicit stems use the existing capture block-ID alphabet. Generated
/// filename stems are intentionally allowed to contain underscores.
pub(crate) fn is_valid_explicit_stem(stem: &str) -> bool {
    is_valid_block_id('^', stem)
}

#[cfg(test)]
mod tests {
    use super::{is_valid_explicit_stem, normalize_stem};

    #[test]
    fn normalizes_unicode_punctuation_and_ampersand_boundaries() {
        assert_eq!(normalize_stem("Crème brûlée"), Some("creme_brulee".into()));
        assert_eq!(
            normalize_stem("King’s Street Coffee"),
            Some("kings_street_coffee".into())
        );
        assert_eq!(
            normalize_stem("Chat & Griddle"),
            Some("chat_and_griddle".into())
        );
        assert_eq!(normalize_stem("&grain"), Some("and_grain".into()));
        assert_eq!(normalize_stem("A&B"), Some("a_and_b".into()));
        assert_eq!(normalize_stem("!!!"), None);
    }

    #[test]
    fn explicit_stem_uses_block_id_alphabet_without_changing_it() {
        assert!(is_valid_explicit_stem("talk-python"));
        assert!(is_valid_explicit_stem("Techmeme"));
        assert!(!is_valid_explicit_stem("talk_python"));
        assert!(!is_valid_explicit_stem(""));
    }
}
