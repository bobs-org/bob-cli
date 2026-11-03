//! Shared Obsidian spellings and parsing for explicit clock tags.

pub(crate) const OFF_TAG: &str = "#off";
pub(crate) const ON_TAG: &str = "#on";
pub(crate) const DEPRECATED_OFF_TAGS: &[&str] =
    &["#offclock", "#off-clock", "#untracked"];

/// Whether a type name is reserved for an explicit clock tag. Keep this
/// alongside the clock spelling table so config and type-edit commands use
/// exactly the same vocabulary.
pub(crate) fn is_reserved_type_name(name: &str) -> bool {
    let key = crate::native::plan_budget::normalize_component(name);
    [OFF_TAG, ON_TAG]
        .into_iter()
        .chain(DEPRECATED_OFF_TAGS.iter().copied())
        .any(|tag| key == tag.trim_start_matches('#'))
}

/// Match a capture-edge token using the same case and right punctuation
/// boundary rules as Obsidian clock tags. `start` points at the `#`.
pub(crate) fn capture_edge_tag_end(text: &str, start: usize) -> Option<usize> {
    let tail = text.get(start..)?;
    if !tail.starts_with('#') {
        return None;
    }
    let token_end = tail.find(char::is_whitespace).unwrap_or(tail.len());
    let token = &tail[..token_end];
    let matched = [OFF_TAG, ON_TAG]
        .into_iter()
        .chain(DEPRECATED_OFF_TAGS.iter().copied())
        .find(|tag| {
            token
                .get(..tag.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(tag))
                && token
                    .get(tag.len()..)
                    .and_then(|rest| rest.chars().next())
                    .is_none_or(is_tag_right_boundary)
        })?;
    let end = start + matched.len();
    Some(end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockKind {
    None,
    Off,
    On,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ClockTag {
    #[default]
    None,
    Off {
        deprecated: Vec<String>,
    },
    On,
    Conflict {
        deprecated: Vec<String>,
    },
}

impl ClockTag {
    /// True only when an explicit off tag wins. `#on` wins a conflict.
    pub(crate) fn explicit_off(&self) -> bool {
        matches!(self, Self::Off { .. })
    }

    pub(crate) fn is_on(&self) -> bool {
        matches!(self, Self::On | Self::Conflict { .. })
    }

    pub(crate) fn kind(&self) -> ClockKind {
        match self {
            Self::None => ClockKind::None,
            Self::Off { .. } => ClockKind::Off,
            Self::On | Self::Conflict { .. } => ClockKind::On,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn deprecated(&self) -> &[String] {
        match self {
            Self::Off { deprecated } | Self::Conflict { deprecated } => {
                deprecated
            }
            Self::None | Self::On => &[],
        }
    }
}

/// Parse one whole Google title or directive token. Google accepts a
/// narrower token grammar than vault lines: the caller whitespace-splits
/// first, then this function strips only trailing punctuation.
pub(crate) fn google_clock_token(token: &str) -> Option<ClockTag> {
    let token = token.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    if token.eq_ignore_ascii_case(ON_TAG) {
        return Some(ClockTag::On);
    }
    if token.eq_ignore_ascii_case(OFF_TAG) {
        return Some(ClockTag::Off {
            deprecated: Vec::new(),
        });
    }
    DEPRECATED_OFF_TAGS
        .iter()
        .find(|tag| token.eq_ignore_ascii_case(tag))
        .map(|tag| ClockTag::Off {
            deprecated: vec![(*tag).to_string()],
        })
}

/// Combine independent title/description Google tag parses. Any `#on`
/// wins, while deprecated alias evidence remains available to diagnostics.
pub(crate) fn merge_clock_tags(left: &ClockTag, right: &ClockTag) -> ClockTag {
    let has_on = left.is_on() || right.is_on();
    let has_off =
        matches!(left, ClockTag::Off { .. } | ClockTag::Conflict { .. })
            || matches!(
                right,
                ClockTag::Off { .. } | ClockTag::Conflict { .. }
            );
    let mut deprecated = Vec::new();
    for token in left.deprecated().iter().chain(right.deprecated()) {
        if !deprecated.contains(token) {
            deprecated.push(token.clone());
        }
    }
    match (has_on, has_off) {
        (false, false) => ClockTag::None,
        (false, true) => ClockTag::Off { deprecated },
        (true, false) => ClockTag::On,
        (true, true) => ClockTag::Conflict { deprecated },
    }
}

/// Parse standalone clock tags outside code spans and Markdown links.
/// Matching is ASCII case-insensitive and follows the vault-line boundary
/// rules: whitespace on the left, and whitespace or closing punctuation on
/// the right.
pub(crate) fn line_clock_tag(line: &str) -> ClockTag {
    let masked = mask_code_and_markdown_links(line);
    let lower = masked.to_ascii_lowercase();
    let mut found = Vec::<(usize, &'static str)>::new();
    for token in [OFF_TAG, ON_TAG]
        .into_iter()
        .chain(DEPRECATED_OFF_TAGS.iter().copied())
    {
        let mut search = 0;
        while let Some(relative) = lower[search..].find(token) {
            let start = search + relative;
            let end = start + token.len();
            let before = masked[..start].chars().next_back();
            let after = masked.get(end..).and_then(|rest| rest.chars().next());
            let left_ok = before.is_none_or(char::is_whitespace);
            let right_ok = after.is_none_or(is_tag_right_boundary);
            if left_ok && right_ok {
                found.push((start, token));
            }
            search = start + 1;
        }
    }
    found.sort_by_key(|(start, _)| *start);

    let has_on = found.iter().any(|(_, token)| *token == ON_TAG);
    let has_off = found.iter().any(|(_, token)| {
        *token == OFF_TAG || DEPRECATED_OFF_TAGS.contains(token)
    });
    let mut deprecated = Vec::new();
    for (_, token) in found {
        if DEPRECATED_OFF_TAGS.contains(&token)
            && !deprecated.iter().any(|seen| seen == token)
        {
            deprecated.push(token.to_string());
        }
    }

    match (has_on, has_off) {
        (false, false) => ClockTag::None,
        (false, true) => ClockTag::Off { deprecated },
        (true, false) => ClockTag::On,
        (true, true) => ClockTag::Conflict { deprecated },
    }
}

/// Remove one trailing standalone clock tag and its punctuation. Repeated
/// callers strip a run such as `#off #on` from an Event Task title.
pub(crate) fn strip_trailing_clock_tag(title: &str) -> Option<&str> {
    let trimmed = title.trim_end();
    let candidate = trimmed
        .trim_end_matches(is_tag_trailing_punctuation)
        .trim_end();
    let masked = mask_code_and_markdown_links(candidate);
    let lower = masked.to_ascii_lowercase();
    for token in [ON_TAG, OFF_TAG]
        .into_iter()
        .chain(DEPRECATED_OFF_TAGS.iter().copied())
    {
        let Some(start) = lower.rfind(token) else {
            continue;
        };
        if start + token.len() == candidate.len()
            && masked[..start]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
        {
            return Some(candidate[..start].trim_end());
        }
    }
    None
}

fn is_tag_right_boundary(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(ch, ']' | ')' | '}' | ':' | '.' | ',' | ';' | '!' | '?')
}

fn is_tag_trailing_punctuation(ch: char) -> bool {
    matches!(ch, ']' | ')' | '}' | ':' | '.' | ',' | ';' | '!' | '?')
}

fn mask_code_and_markdown_links(line: &str) -> String {
    let mut out = line.as_bytes().to_vec();
    let mut index = 0;
    while index < line.len() {
        if line.as_bytes()[index] == b'`' {
            let width = line.as_bytes()[index..]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            if let Some(close) = find_code_close(line, index + width, width) {
                blank(&mut out, index, close);
                index = close;
                continue;
            }
        }
        if line.as_bytes()[index] == b'['
            && let Some(end) = markdown_link_end(line, index)
        {
            blank(&mut out, index, end);
            index = end;
            continue;
        }
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| line.to_string())
}

fn find_code_close(line: &str, start: usize, width: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut search = start;
    while search < bytes.len() {
        if bytes[search] != b'`' {
            search += 1;
            continue;
        }
        let close = search;
        while search < bytes.len() && bytes[search] == b'`' {
            search += 1;
        }
        if search - close == width {
            return Some(search);
        }
    }
    None
}

fn markdown_link_end(line: &str, open: usize) -> Option<usize> {
    let rest = line.get(open + 1..)?;
    let text_close = rest.find(']')?;
    let after = rest.get(text_close + 1..)?;
    if !after.starts_with('(') {
        return None;
    }
    let url_close = after.get(1..)?.find(')')?;
    Some(open + 1 + text_close + 1 + 1 + url_close + 1)
}

fn blank(bytes: &mut [u8], start: usize, end: usize) {
    for byte in bytes.iter_mut().take(end).skip(start) {
        *byte = b' ';
    }
}

#[cfg(test)]
mod tests {
    use super::{line_clock_tag, strip_trailing_clock_tag, ClockTag};

    #[test]
    fn parses_clock_spellings_with_boundaries_and_precedence() {
        for tag in ["#off", "#OFF", "#offclock", "#off-clock", "#untracked"] {
            assert!(
                line_clock_tag(&format!("Walk {tag}.")).explicit_off(),
                "{tag}"
            );
        }
        assert_eq!(line_clock_tag("Walk #on"), ClockTag::On);
        assert!(matches!(
            line_clock_tag("Walk #off #on"),
            ClockTag::Conflict { .. }
        ));
        for line in [
            "#off/x",
            "#offclockx #office",
            "#one #online",
            "walk#off",
            "`#off`",
            "[see #off](https://example.com)",
        ] {
            assert_eq!(line_clock_tag(line), ClockTag::None, "{line}");
        }
        assert!(line_clock_tag("Walk #off)").explicit_off());
    }

    #[test]
    fn removes_only_trailing_clock_tags() {
        for tag in ["#off", "#ON.", "#offclock!", "#OFF-CLOCK)", "#untracked"] {
            assert_eq!(
                strip_trailing_clock_tag(&format!("Lunch {tag}")),
                Some("Lunch"),
                "{tag}"
            );
        }
        assert_eq!(strip_trailing_clock_tag("Lunch #office"), None);
        assert_eq!(strip_trailing_clock_tag("Lunch [see #off](url)"), None);
        assert_eq!(strip_trailing_clock_tag("Lunch `#off`"), None);
    }
}
