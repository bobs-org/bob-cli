//! EVENT token grammar for `bob ref meet`.
//!
//! An EVENT names one meeting three ways: an explicit Event Task ref, a
//! Google Calendar URL, or title query words. Explicit forms always win,
//! URLs decode through the journal, and bare words use the `bob gcal note`
//! rule (every word appears in the title, case folded).

/// One parsed EVENT token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventToken {
    /// A direct Event Task ref: `20261123#^id`, `2026/20261123#^id`,
    /// `[[20261123#^id|alias]]`, `20261123:id`, or any of those with a
    /// leading `~`.
    Explicit { note: String, block_id: String },
    /// A Google Calendar URL carrying `eid=`.
    Url { url: String },
    /// Title query words: every word must appear in the title.
    Query { words: Vec<String> },
}

/// Normalize one raw EVENT token to its parsed form.
pub(crate) fn parse_event_token(raw: &str) -> EventToken {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix('~') {
        text = rest.trim();
    }
    if text.contains("eid=") {
        return EventToken::Url {
            url: text.to_string(),
        };
    }
    if let Some(inner) = text
        .strip_prefix("[[")
        .and_then(|rest| rest.find("]]").map(|end| &rest[..end]))
    {
        let target = inner
            .split_once('|')
            .map_or(inner.trim(), |(head, _)| head.trim());
        if let Some((note, id)) = target.split_once("#^")
            && let Some(explicit) = explicit_ref(note, id)
        {
            return explicit;
        }
        return EventToken::Query {
            words: split_words(target),
        };
    }
    if let Some((note, id)) = text.split_once("#^") {
        if let Some(explicit) = explicit_ref(note, id) {
            return explicit;
        }
        return EventToken::Query {
            words: split_words(text),
        };
    }
    // `note:id` (TaskLocator spelling, with or without the `^`); only when
    // the tail is a plausible block id so URLs and prose with colons fall
    // through to query words.
    if let Some((note, id)) = text.rsplit_once(':')
        && !note.contains("://")
        && plausible_block_id(id)
        && !note.trim().is_empty()
    {
        let id = id.strip_prefix('^').unwrap_or(id);
        return EventToken::Explicit {
            note: note.trim().to_string(),
            block_id: id.to_string(),
        };
    }
    EventToken::Query {
        words: split_words(text),
    }
}

/// Split query text on whitespace, dropping empties.
fn split_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// Whether `id` is shaped like a block id: nonempty, no whitespace, and
/// none of the locator metacharacters.
fn plausible_block_id(id: &str) -> bool {
    let id = id.strip_prefix('^').unwrap_or(id);
    !id.is_empty()
        && !id.chars().any(char::is_whitespace)
        && !id.contains(['#', '^', '[', ']'])
        && !id.contains('/')
}

/// Build an explicit ref from a `note#^id` split, or `None` when either
/// side is malformed (the caller falls back to query words).
fn explicit_ref(note: &str, id: &str) -> Option<EventToken> {
    let note = note.trim();
    let id = id.trim();
    if note.is_empty() || !plausible_block_id(id) {
        return None;
    }
    let id = id.strip_prefix('^').unwrap_or(id);
    Some(EventToken::Explicit {
        note: note.to_string(),
        block_id: id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_explicit_spelling() {
        for raw in [
            "20261123#^standup",
            "2026/20261123#^standup",
            "[[20261123#^standup|Standup]]",
            "[[2026/20261123#^standup]]",
            "20261123:standup",
            "20261123:^standup",
            "~20261123:standup",
            "~20261123#^standup",
            "  20261123#^standup  ",
        ] {
            let token = parse_event_token(raw);
            match token {
                EventToken::Explicit { note, block_id } => {
                    assert_eq!(block_id, "standup", "{raw}");
                    assert!(
                        note.contains("20261123"),
                        "{raw} kept its note: {note}"
                    );
                }
                other => panic!("{raw} parsed as {other:?}"),
            }
        }
    }

    #[test]
    fn parses_urls_and_queries() {
        match parse_event_token(
            "https://calendar.google.com/event?eid=abc123 \"Open\"",
        ) {
            EventToken::Url { url } => assert!(url.contains("eid=")),
            other => panic!("url parsed as {other:?}"),
        }
        match parse_event_token("standup Kelly") {
            EventToken::Query { words } => {
                assert_eq!(words, vec!["standup", "Kelly"]);
            }
            other => panic!("query parsed as {other:?}"),
        }
        match parse_event_token("") {
            EventToken::Query { words } => assert!(words.is_empty()),
            other => panic!("empty parsed as {other:?}"),
        }
        // Prose with a colon is a query, not a locator.
        match parse_event_token("standup: the recap") {
            EventToken::Query { words } => assert!(!words.is_empty()),
            other => panic!("prose parsed as {other:?}"),
        }
    }
}
