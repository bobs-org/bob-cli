use super::{
    parse_dossier, ContentRef, MeetingNextStep, MeetingNotes, MeetingSection,
    NotesShape,
};

const UNRECOGNIZED_WARNING: &str =
    "unrecognized notes shape; filed as plain paragraphs";

pub(crate) fn normalize(text: &str) -> MeetingNotes {
    if let Ok(document) = parse_dossier(text) {
        return document.notes;
    }

    let text = canonical_text(text);
    let mut notes = MeetingNotes::default();
    if is_markdown(&text) {
        notes.shape = NotesShape::Markdown;
        normalize_markdown(&text, &mut notes);
    } else {
        let (blocks, has_wispr_sections) = split_wispr_blocks(&text);
        if has_wispr_sections {
            notes.shape = NotesShape::WisprPlainText;
            normalize_wispr(blocks, &mut notes);
        } else {
            notes.tldr = text.trim().to_string();
            notes.warnings.push(UNRECOGNIZED_WARNING.to_string());
        }
    }
    notes
}

fn canonical_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .lines()
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_markdown(text: &str) -> bool {
    let mut list_items = 0;
    for line in text.lines() {
        if atx_heading(line).is_some() {
            return true;
        }
        if strip_list_marker(line).is_some() {
            list_items += 1;
            if list_items >= 2 {
                return true;
            }
        }
    }
    false
}

fn normalize_markdown(text: &str, notes: &mut MeetingNotes) {
    let mut pending = Vec::new();
    let mut active = Active::Tldr;
    let mut saw_content = false;

    for line in text.lines() {
        if let Some((level, heading)) = atx_heading(line) {
            flush_markdown_block(&pending, &active, notes);
            pending.clear();

            if level == 1 && notes.own_title.is_none() && !saw_content {
                notes.own_title = Some(heading.to_string());
                continue;
            }
            if (2..=3).contains(&level) {
                if is_transcript_heading(heading) {
                    active = Active::Transcript;
                    if notes.transcript.is_none() {
                        notes.transcript = Some(String::new());
                    }
                } else if is_next_steps_heading(heading) {
                    notes.next_steps_heading = Some(heading.to_string());
                    notes.next_steps_level = level;
                    notes.content_order.push(ContentRef::NextSteps);
                    active = Active::NextSteps;
                } else {
                    let index = notes.sections.len();
                    notes.sections.push(MeetingSection {
                        heading: Some(heading.to_string()),
                        level,
                        items: Vec::new(),
                    });
                    notes.content_order.push(ContentRef::Section(index));
                    active = Active::Section(index);
                }
                saw_content = true;
                continue;
            }
            // A later H1 remains part of the notes instead of being mistaken for the
            // title-card title.
            pending.push(line.to_string());
            saw_content = true;
            continue;
        }
        if !line.trim().is_empty() {
            saw_content = true;
        }
        pending.push(line.to_string());
    }
    flush_markdown_block(&pending, &active, notes);
    notes.tldr = notes.tldr.trim().to_string();
    if let Some(transcript) = notes.transcript.as_mut() {
        *transcript = transcript.trim().to_string();
        if transcript.is_empty() {
            notes.transcript = None;
        }
    }
}

#[derive(Clone, Copy)]
enum Active {
    Tldr,
    Section(usize),
    NextSteps,
    Transcript,
}

fn flush_markdown_block(
    lines: &[String],
    active: &Active,
    notes: &mut MeetingNotes,
) {
    let content = lines.join("\n").trim().to_string();
    if content.is_empty() {
        return;
    }
    match active {
        Active::Tldr => append_paragraph(&mut notes.tldr, &content),
        Active::Transcript => {
            let transcript = notes.transcript.get_or_insert_with(String::new);
            append_paragraph(transcript, &content);
        }
        Active::Section(index) => {
            if let Some(section) = notes.sections.get_mut(*index) {
                section.items.extend(markdown_items(&content));
            }
        }
        Active::NextSteps => {
            notes.next_steps.extend(
                markdown_items(&content).into_iter().map(parse_next_step),
            );
        }
    }
}

fn markdown_items(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() > 1
        && lines.iter().all(|line| strip_list_marker(line).is_some())
    {
        return lines
            .into_iter()
            .filter_map(strip_list_marker)
            .map(str::to_string)
            .collect();
    }
    if lines.len() == 1 {
        return vec![strip_list_marker(lines[0])
            .unwrap_or(lines[0])
            .to_string()];
    }
    vec![content.to_string()]
}

fn normalize_wispr(blocks: Vec<String>, notes: &mut MeetingNotes) {
    let Some(first) = blocks.first() else {
        return;
    };
    notes.tldr = first.trim().to_string();
    let mut active_section: Option<usize> = None;
    let mut active_next_steps = false;

    for block in blocks.iter().skip(1) {
        let paragraphs = split_single_blank_paragraphs(block);
        if paragraphs.is_empty() {
            continue;
        }
        let first_paragraph = paragraphs[0].trim();
        if first_paragraph.contains('\n') || first_paragraph.contains('\r') {
            let mut lines = first_paragraph.lines();
            let first_line = lines.next().unwrap_or("").trim();
            if is_short_heading(first_line) && is_next_steps_heading(first_line)
            {
                notes.next_steps_heading = Some(first_line.to_string());
                notes.next_steps_level = 2;
                notes.content_order.push(ContentRef::NextSteps);
                active_next_steps = true;
                active_section = None;
                for line in lines {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    notes.next_steps.push(parse_next_step(line.to_owned()));
                }
                for paragraph in paragraphs.iter().skip(1) {
                    push_wispr_item(
                        paragraph,
                        active_section,
                        active_next_steps,
                        notes,
                    );
                }
                continue;
            }
            for paragraph in &paragraphs {
                push_wispr_item(
                    paragraph,
                    active_section,
                    active_next_steps,
                    notes,
                );
            }
            continue;
        }
        if is_short_heading(first_paragraph) {
            if is_next_steps_heading(first_paragraph) {
                notes.next_steps_heading = Some(first_paragraph.to_string());
                notes.next_steps_level = 2;
                notes.content_order.push(ContentRef::NextSteps);
                active_next_steps = true;
                active_section = None;
            } else {
                let index = notes.sections.len();
                notes.sections.push(MeetingSection {
                    heading: Some(first_paragraph.to_string()),
                    level: 2,
                    items: Vec::new(),
                });
                notes.content_order.push(ContentRef::Section(index));
                active_section = Some(index);
                active_next_steps = false;
            }
            for paragraph in paragraphs.iter().skip(1) {
                push_wispr_item(
                    paragraph,
                    active_section,
                    active_next_steps,
                    notes,
                );
            }
        } else {
            for paragraph in &paragraphs {
                push_wispr_item(
                    paragraph,
                    active_section,
                    active_next_steps,
                    notes,
                );
            }
        }
    }
}

fn push_wispr_item(
    paragraph: &str,
    active_section: Option<usize>,
    active_next_steps: bool,
    notes: &mut MeetingNotes,
) {
    let paragraph = paragraph.trim();
    if paragraph.is_empty() {
        return;
    }
    if active_next_steps {
        for line in paragraph
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            notes.next_steps.push(parse_next_step(line.to_owned()));
        }
    } else {
        let index = active_section.unwrap_or_else(|| {
            let index = notes.sections.len();
            notes.sections.push(MeetingSection {
                heading: None,
                level: 2,
                items: Vec::new(),
            });
            notes.content_order.push(ContentRef::Section(index));
            index
        });
        if let Some(section) = notes.sections.get_mut(index) {
            section.items.push(paragraph.to_string());
        }
    }
}

fn split_wispr_blocks(text: &str) -> (Vec<String>, bool) {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut blank_run = 0usize;
    let mut found_boundary = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            continue;
        }
        if blank_run >= 2 {
            let block = current.join("\n").trim().to_string();
            if !block.is_empty() {
                blocks.push(block);
            }
            current.clear();
            found_boundary = true;
        } else if blank_run == 1 && !current.is_empty() {
            current.push(String::new());
        }
        blank_run = 0;
        current.push(line.to_string());
    }
    let final_block = current.join("\n").trim().to_string();
    if !final_block.is_empty() {
        blocks.push(final_block);
    }
    (blocks, found_boundary)
}

fn split_single_blank_paragraphs(block: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = Vec::new();
    for line in block.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                result.push(current.join("\n").trim().to_string());
                current.clear();
            }
        } else {
            current.push(line.to_string());
        }
    }
    if !current.is_empty() {
        result.push(current.join("\n").trim().to_string());
    }
    result
}

fn is_short_heading(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return false;
    }
    let word_count = trimmed.split_whitespace().count();
    !trimmed.is_empty()
        && trimmed.len() <= 96
        && word_count <= 12
        && !trimmed
            .chars()
            .last()
            .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | ':' | ';'))
}

fn is_next_steps_heading(heading: &str) -> bool {
    let normalized = heading.trim().trim_end_matches(':').to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "next steps" | "action items" | "follow-ups" | "follow ups"
    )
}

fn is_transcript_heading(heading: &str) -> bool {
    heading.trim().eq_ignore_ascii_case("transcript")
}

fn parse_next_step(text: String) -> MeetingNextStep {
    let text = strip_list_marker(&text).unwrap_or(&text).trim();
    if let Some(after_open) = text.strip_prefix('(')
        && let Some((owner, action)) = after_open.split_once(')')
        && !owner.trim().is_empty()
        && action.starts_with(char::is_whitespace)
    {
        return MeetingNextStep {
            owner: Some(owner.trim().to_string()),
            action: action.trim_start().to_string(),
        };
    }
    MeetingNextStep {
        owner: None,
        action: text.to_string(),
    }
}

fn strip_list_marker(text: &str) -> Option<&str> {
    let text = text.trim_start();
    for marker in [
        "- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "* [X] ", "+ [ ] ",
        "+ [x] ", "+ [X] ",
    ] {
        if let Some(rest) = text.strip_prefix(marker) {
            return Some(rest);
        }
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = text.strip_prefix(marker) {
            return Some(rest);
        }
    }
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let rest = &text[digits..];
        if let Some(rest) =
            rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
        {
            return Some(rest);
        }
    }
    None
}

fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim_end();
    Some((level as u8, title))
}

fn append_paragraph(target: &mut String, content: &str) {
    if !target.is_empty() {
        target.push_str("\n\n");
    }
    target.push_str(content.trim());
}
