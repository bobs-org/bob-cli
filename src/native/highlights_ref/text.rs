//! PDF text cleanup and normalization.

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
