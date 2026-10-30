//! Relation-label normalisation (`docs/task-relations.md` §3.2).

pub(crate) const MAX_LABEL_CHARS: usize = 40;

const FORBIDDEN_CHARS: &[char] =
    &['[', ']', '(', ')', '"', '\\', '|', '•', '*', '~'];

/// Why a writer-side label was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelError {
    Empty,
    TooLong,
    ForbiddenCharacter,
    InvalidStart,
    ReservedMention,
}

impl LabelError {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLong => "too long",
            Self::ForbiddenCharacter => "character",
            Self::InvalidStart => "start",
            Self::ReservedMention => "💬 is reserved for text mentions",
        }
    }
}

/// Writer normalisation: trim, collapse whitespace, strip one trailing
/// `:`, then require 1–40 characters that obey the character rules.
pub(crate) fn normalize_relation_label(
    raw: &str,
) -> Result<String, LabelError> {
    let collapsed = collapse_label_text(raw);
    if collapsed.is_empty() {
        return Err(LabelError::Empty);
    }
    if is_reserved_mention_label(&collapsed) {
        return Err(LabelError::ReservedMention);
    }
    validate_label_characters(&collapsed)?;
    if collapsed.chars().count() > MAX_LABEL_CHARS {
        return Err(LabelError::TooLong);
    }
    Ok(collapsed)
}

/// Whether a normalized label uses the reserved mention namespace.
pub(crate) fn is_reserved_mention_label(label: &str) -> bool {
    label.starts_with('💬')
}

/// Capture spelling of a canonical label: `|label` or `|"multi word"`.
#[allow(dead_code)] // capture-complete replacement spelling
pub(crate) fn relation_label_quoted_form(label: &str) -> String {
    if label.contains(' ') {
        format!("|\"{label}\"")
    } else {
        format!("|{label}")
    }
}

/// The `||` spelling of a canonical label: `||label` or `||"multi word"`.
/// The stored Markdown is identical to the `|` form; `||` additionally
/// hides the label from suggestions once the capture succeeds.
pub(crate) fn relation_label_hide_form(label: &str) -> String {
    if label.contains(' ') {
        format!("||\"{label}\"")
    } else {
        format!("||{label}")
    }
}

/// Display-only inverse phrase for a stored label
/// (`docs/task-relations.md` §10), matched case-insensitively and
/// never stored.
pub(crate) fn inverse_phrase(label: &str) -> Option<&'static str> {
    const INVERSES: &[(&str, &str)] = &[
        ("merged into", "merged from"),
        ("duplicate of", "duplicated by"),
        ("follow-up of", "followed up by"),
        ("superseded by", "supersedes"),
        ("split from", "split into"),
        ("implemented by", "implements"),
        ("verified by", "verifies"),
    ];
    let collapsed = collapse_label_text(label);
    INVERSES.iter().find_map(|(stored, inverse)| {
        stored.eq_ignore_ascii_case(&collapsed).then_some(*inverse)
    })
}

/// Reader-side label: the same collapse and character rules, any
/// length. Empty after collapse is "no label".
pub(crate) fn parse_relation_label(
    raw: &str,
) -> Result<Option<String>, LabelError> {
    let collapsed = collapse_label_text(raw);
    if collapsed.is_empty() {
        return Ok(None);
    }
    validate_label_characters(&collapsed)?;
    Ok(Some(collapsed))
}

pub(crate) fn collapse_label_text(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let stripped = collapsed.strip_suffix(':').unwrap_or(&collapsed);
    stripped.trim().to_string()
}

fn validate_label_characters(label: &str) -> Result<(), LabelError> {
    if label.starts_with('·') || label.starts_with(',') {
        return Err(LabelError::InvalidStart);
    }
    if label.chars().any(|character| {
        character.is_control() || FORBIDDEN_CHARS.contains(&character)
    }) {
        return Err(LabelError::ForbiddenCharacter);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lw11_normalizes_and_refuses() {
        assert_eq!(
            normalize_relation_label(" Follow-up  of:").unwrap(),
            "Follow-up of"
        );
        let too_long = "a".repeat(41);
        assert_eq!(
            normalize_relation_label(&too_long),
            Err(LabelError::TooLong)
        );
        assert_eq!(
            normalize_relation_label("see (old)"),
            Err(LabelError::ForbiddenCharacter)
        );
    }

    #[test]
    fn quoted_form_uses_quotes_for_spaces() {
        assert_eq!(relation_label_quoted_form("see"), "|see");
        assert_eq!(
            relation_label_quoted_form("follow-up of"),
            "|\"follow-up of\""
        );
    }

    #[test]
    fn readers_accept_long_labels() {
        let long = "a".repeat(41);
        assert_eq!(
            parse_relation_label(&long).unwrap().as_deref(),
            Some(&*long)
        );
    }
}
