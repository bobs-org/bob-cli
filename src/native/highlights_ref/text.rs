//! PDF text cleanup and normalization.

use super::return_links::TAG_GLYPHS;

pub(super) struct ReflowLine {
    pub(super) text: String,
    pub(super) ended_with_soft_hyphen: bool,
}

pub(super) fn clean_pdf_text_artifacts(text: &str) -> String {
    text.split('\n')
        .map(clean_pdf_text_artifacts_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn clean_pdf_text_artifacts_line(line: &str) -> String {
    let mut cleaned = String::new();
    let mut pending_space = false;
    for character in line.chars() {
        match character {
            '\u{fb00}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ff")
            }
            '\u{fb01}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "fi")
            }
            '\u{fb02}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "fl")
            }
            '\u{fb03}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ffi")
            }
            '\u{fb04}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ffl")
            }
            '\u{fb05}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ft")
            }
            '\u{fb06}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "st")
            }
            '\u{00ad}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}'
            | '\u{feff}' => {}
            ' ' | '\t' | '\r' | '\u{00a0}' | '\u{2007}' | '\u{202f}' => {
                pending_space = true
            }
            character if character.is_whitespace() => pending_space = true,
            character => {
                if pending_space && !cleaned.is_empty() {
                    cleaned.push(' ');
                }
                pending_space = false;
                cleaned.push(character);
            }
        }
    }
    cleaned
}

pub(super) fn push_cleaned_fragment(
    cleaned: &mut String,
    pending_space: &mut bool,
    fragment: &str,
) {
    if *pending_space && !cleaned.is_empty() {
        cleaned.push(' ');
    }
    *pending_space = false;
    cleaned.push_str(fragment);
}

pub(super) fn beautify_annotation_text(text: &str) -> String {
    let cleaned_text = clean_pdf_text_artifacts(text);
    let mut rendered = Vec::new();
    let mut current = None::<ReflowLine>;

    for (raw_line, line) in text.split('\n').zip(cleaned_text.split('\n')) {
        let line = line.trim().to_string();
        let ended_with_soft_hyphen = ends_with_soft_hyphen(raw_line);

        if line.is_empty() {
            flush_reflow_line(&mut rendered, &mut current);
            if !rendered.is_empty()
                && !rendered.last().is_some_and(String::is_empty)
            {
                rendered.push(String::new());
            }
            continue;
        }

        if is_markdown_unordered_list_line(&line) {
            flush_reflow_line(&mut rendered, &mut current);
            current = Some(ReflowLine {
                text: line,
                ended_with_soft_hyphen,
            });
            continue;
        }

        if let Some(current) = &mut current {
            append_reflow_fragment(
                &mut current.text,
                &line,
                current.ended_with_soft_hyphen,
            );
            current.ended_with_soft_hyphen = ended_with_soft_hyphen;
        } else {
            current = Some(ReflowLine {
                text: line,
                ended_with_soft_hyphen,
            });
        }
    }

    flush_reflow_line(&mut rendered, &mut current);
    while rendered.last().is_some_and(String::is_empty) {
        rendered.pop();
    }
    rendered.join("\n")
}

pub(super) fn flush_reflow_line(
    rendered: &mut Vec<String>,
    current: &mut Option<ReflowLine>,
) {
    if let Some(line) = current.take() {
        rendered.push(line.text);
    }
}

pub(super) fn append_reflow_fragment(
    current: &mut String,
    fragment: &str,
    previous_ended_with_soft_hyphen: bool,
) {
    if previous_ended_with_soft_hyphen {
        current.push_str(fragment);
        return;
    }

    let mut current_chars = current.chars().rev();
    if let Some(last) = current_chars.next()
        && matches!(last, '-' | '‐')
        && current_chars.next().is_some_and(char::is_alphabetic)
        && let Some(first) = fragment.chars().next()
    {
        if first.is_lowercase() {
            current.pop();
            current.push_str(fragment);
            return;
        }
        if first.is_uppercase() || first.is_ascii_digit() {
            current.push_str(fragment);
            return;
        }
    }

    if !current.is_empty() {
        current.push(' ');
    }
    current.push_str(fragment);
}

pub(super) fn ends_with_soft_hyphen(line: &str) -> bool {
    line.trim_end_matches(|character| {
        matches!(
            character,
            ' ' | '\t' | '\r' | '\u{00a0}' | '\u{2007}' | '\u{202f}'
        )
    })
    .ends_with('\u{00ad}')
}

pub(super) fn is_markdown_unordered_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let mut characters = trimmed.chars();
    matches!(characters.next(), Some('-' | '*' | '+'))
        && characters.next().is_some_and(char::is_whitespace)
}

pub(super) fn normalized_identity_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_tag_glyph(character: char) -> bool {
    TAG_GLYPHS.contains(&character)
}

/// Strip paired-return-link artifacts from highlight text, for PDFs stamped
/// with `return_links: true` (Highlights exports exactly what is drawn, and
/// PDFKit ignores `/ActualText`). Pill `↩ p. N<tags>` fragments go first,
/// each with one adjacent whitespace run so neighbours stay single-spaced;
/// then maximal modifier-letter tag runs that directly follow a
/// non-whitespace character or start the text. Everything else is left for
/// [`beautify_annotation_text`].
pub(super) fn strip_return_link_glyphs(text: &str) -> String {
    strip_tag_runs(&strip_pill_fragments(text))
}

fn char_byte_offsets(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect()
}

/// Match `↩[ws]p.[ws]digits[ws]tags` at `chars[start]` (`↩`); the returned
/// index is the exclusive char end of the fragment.
fn match_pill_fragment(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    while chars.get(index).is_some_and(|cell| cell.is_whitespace()) {
        index += 1;
    }
    if chars.get(index) != Some(&'p') {
        return None;
    }
    index += 1;
    if chars.get(index) != Some(&'.') {
        return None;
    }
    index += 1;
    while chars.get(index).is_some_and(|cell| cell.is_whitespace()) {
        index += 1;
    }
    let digits = index;
    while chars.get(index).is_some_and(|cell| cell.is_ascii_digit()) {
        index += 1;
    }
    if index == digits {
        return None;
    }
    while chars.get(index).is_some_and(|cell| cell.is_whitespace()) {
        index += 1;
    }
    let tags = index;
    while chars.get(index).is_some_and(|cell| is_tag_glyph(*cell)) {
        index += 1;
    }
    (index != tags).then_some(index)
}

fn is_whitespace_char(character: char) -> bool {
    character.is_whitespace()
}

fn strip_pill_fragments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let offsets = char_byte_offsets(text);
    let mut fragments: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '↩'
            && let Some(end) = match_pill_fragment(&chars, index)
        {
            fragments.push((offsets[index], offsets[end]));
            index = end;
            continue;
        }
        index += 1;
    }
    let mut stripped = String::new();
    let mut cursor = 0;
    for (start, end) in fragments {
        let gap = &text[cursor..start];
        let after = &text[end..];
        if after.starts_with(is_whitespace_char) {
            stripped.push_str(gap);
            cursor =
                text.len() - after.trim_start_matches(is_whitespace_char).len();
        } else if gap.ends_with(is_whitespace_char) {
            stripped.push_str(gap.trim_end_matches(is_whitespace_char));
            cursor = end;
        } else {
            stripped.push_str(gap);
            cursor = end;
        }
    }
    stripped.push_str(&text[cursor..]);
    stripped
}

fn strip_tag_runs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let offsets = char_byte_offsets(text);
    let mut stripped = String::new();
    let mut cursor = 0;
    let mut index = 0;
    while index < chars.len() {
        let at_run_start = is_tag_glyph(chars[index])
            && (index == 0 || !chars[index - 1].is_whitespace());
        if at_run_start {
            stripped.push_str(&text[offsets[cursor]..offsets[index]]);
            while index < chars.len() && is_tag_glyph(chars[index]) {
                index += 1;
            }
            cursor = index;
        } else {
            index += 1;
        }
    }
    stripped.push_str(&text[offsets[cursor]..]);
    stripped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_return_link_glyphs_covers_every_shape() {
        assert_eq!(
            strip_return_link_glyphs(
                "grows about 2–3 tasks a day (What I verifiedᵈ, row 12)"
            ),
            "grows about 2–3 tasks a day (What I verified, row 12)"
        );
        assert_eq!(
            strip_return_link_glyphs("see the rankingᵃᵈ."),
            "see the ranking."
        );
        assert_eq!(strip_return_link_glyphs("↩ p. 2ᵈ ↩ p. 4ⁱ ↩ p. 13ⁿ"), "");
        assert_eq!(strip_return_link_glyphs("↩ p.2 ᵈ"), "");
        assert_eq!(
            strip_return_link_glyphs(
                "1.4 What I verified ↩ p. 2ᵈ Text continues"
            ),
            "1.4 What I verified Text continues"
        );
        assert_eq!(strip_return_link_glyphs("ᵈ, row 12"), ", row 12");
        // With the flag on, even phonetic modifier letters strip: only the
        // flag being off (no call) keeps `pʰ` intact.
        assert_eq!(strip_return_link_glyphs("aspirated pʰ"), "aspirated p");
        assert_eq!(
            strip_return_link_glyphs("plain words stay untouched"),
            "plain words stay untouched"
        );
        // A bare `↩` and a spaced tag glyph are not pill/tag shapes.
        assert_eq!(strip_return_link_glyphs("go ↩ back"), "go ↩ back");
        assert_eq!(strip_return_link_glyphs("tag ᵈ alone"), "tag ᵈ alone");
    }
}
