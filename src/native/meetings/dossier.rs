use std::{error::Error as StdError, fmt};

use serde::{Deserialize, Serialize};

use super::{
    ContentRef, DossierMeta, MeetingNextStep, MeetingNotes, MeetingSection,
    NotesShape,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DossierDocument {
    pub(crate) notes: MeetingNotes,
    pub(crate) meta: DossierMeta,
}

#[derive(Debug)]
pub(crate) enum DossierError {
    MissingFrontmatter,
    InvalidFrontmatter(serde_yaml::Error),
    UnsupportedVersion(u8),
}

impl fmt::Display for DossierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => {
                write!(f, "meeting dossier front matter is missing")
            }
            Self::InvalidFrontmatter(error) => {
                write!(f, "meeting dossier front matter is invalid: {error}")
            }
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported meeting dossier version {version}")
            }
        }
    }
}

impl StdError for DossierError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::InvalidFrontmatter(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct DossierFrontmatter {
    bob_meeting_dossier: u8,
    event_title: String,
    date: String,
    span: String,
    root: String,
    producer: Option<String>,
    filed_date: String,
    include_transcript: bool,
    edited: bool,
    notes_shape: NotesShape,
    warnings: Vec<String>,
}

pub(crate) fn render_dossier(
    notes: &MeetingNotes,
    meta: &DossierMeta,
) -> String {
    let frontmatter = DossierFrontmatter {
        bob_meeting_dossier: 1,
        event_title: meta.event_title.clone(),
        date: meta.date.clone(),
        span: meta.span.clone(),
        root: meta.root.clone(),
        producer: meta.producer.clone(),
        filed_date: meta.filed_date.clone(),
        include_transcript: meta.include_transcript,
        edited: meta.edited,
        notes_shape: notes.shape,
        warnings: notes.warnings.clone(),
    };
    let yaml = serde_yaml::to_string(&frontmatter)
        .expect("meeting dossier front matter is serializable");
    let mut output = format!("---\n{yaml}---\n");
    render_body(notes, meta, &mut output);
    output
}

pub(crate) fn parse_dossier(
    text: &str,
) -> Result<DossierDocument, DossierError> {
    let text = text
        .strip_prefix("---\n")
        .ok_or(DossierError::MissingFrontmatter)?;
    let (yaml, body) = text
        .split_once("\n---\n")
        .ok_or(DossierError::MissingFrontmatter)?;
    let frontmatter: DossierFrontmatter =
        serde_yaml::from_str(yaml).map_err(DossierError::InvalidFrontmatter)?;
    if frontmatter.bob_meeting_dossier != 1 {
        return Err(DossierError::UnsupportedVersion(
            frontmatter.bob_meeting_dossier,
        ));
    }
    let meta = DossierMeta {
        event_title: frontmatter.event_title,
        date: frontmatter.date,
        span: frontmatter.span,
        root: frontmatter.root,
        producer: frontmatter.producer,
        filed_date: frontmatter.filed_date,
        include_transcript: frontmatter.include_transcript,
        edited: frontmatter.edited,
    };
    let notes = parse_body(body, frontmatter.notes_shape, frontmatter.warnings);
    Ok(DossierDocument { notes, meta })
}

fn render_body(notes: &MeetingNotes, meta: &DossierMeta, output: &mut String) {
    if let Some(title) = notes.own_title.as_deref() {
        output.push_str("# ");
        output.push_str(title);
        output.push_str("\n\n");
    }

    if !notes.tldr.is_empty() {
        render_fenced_div("meeting-tldr", &notes.tldr, output);
    }

    for content in &notes.content_order {
        match *content {
            ContentRef::NextSteps => {
                let level = notes.next_steps_level.clamp(2, 3);
                render_heading(
                    level,
                    notes.next_steps_heading.as_deref().unwrap_or("Next steps"),
                    output,
                );
                for step in &notes.next_steps {
                    let mut line = String::from("- ");
                    if let Some(owner) = step.owner.as_deref() {
                        line.push('(');
                        line.push_str(owner);
                        line.push_str(") ");
                    }
                    line.push_str(&step.action);
                    output.push_str(
                        &line.split_whitespace().collect::<Vec<_>>().join(" "),
                    );
                    output.push('\n');
                }
                output.push('\n');
            }
            ContentRef::Section(index) => {
                let Some(section) = notes.sections.get(index) else {
                    continue;
                };
                if let Some(heading) = section.heading.as_deref() {
                    render_heading(section.level.clamp(2, 3), heading, output);
                }
                for item in &section.items {
                    output.push_str("- ");
                    output.push_str(
                        &item.split_whitespace().collect::<Vec<_>>().join(" "),
                    );
                    output.push('\n');
                }
                output.push('\n');
            }
        }
    }

    if meta.include_transcript
        && let Some(transcript) = notes.transcript.as_deref()
        && !transcript.trim().is_empty()
    {
        render_heading(3, "Transcript", output);
        render_fenced_div("meeting-transcript", transcript, output);
    }
}

fn parse_body(
    body: &str,
    shape: NotesShape,
    warnings: Vec<String>,
) -> MeetingNotes {
    let mut notes = MeetingNotes {
        shape,
        warnings,
        ..MeetingNotes::default()
    };
    let lines: Vec<&str> = body.lines().collect();
    let mut index = 0;
    if let Some(line) = lines.first()
        && let Some(title) = line.strip_prefix("# ")
    {
        notes.own_title = Some(title.to_string());
        index = 1;
    }

    while index < lines.len() {
        if let Some(fence) = tldr_fence(lines[index]) {
            index += 1;
            let mut content = Vec::new();
            while index < lines.len() && lines[index].trim() != fence {
                content.push(lines[index]);
                index += 1;
            }
            notes.tldr = content.join("\n").trim().to_string();
            index = index.saturating_add(1);
            break;
        }
        if !lines[index].trim().is_empty() {
            break;
        }
        index += 1;
    }

    let mut active = BodySection::None;
    let mut transcript = Vec::new();
    while index < lines.len() {
        let line = lines[index];
        if matches!(active, BodySection::Transcript)
            && let Some(fence) = class_fence(line, "meeting-transcript")
        {
            index += 1;
            let mut content = Vec::new();
            while index < lines.len() && lines[index].trim() != fence {
                content.push(lines[index]);
                index += 1;
            }
            transcript.push(content.join("\n").trim().to_string());
            index = index.saturating_add(1);
            continue;
        }
        if matches!(active, BodySection::Transcript) {
            if !line.trim().is_empty() {
                transcript.push(line.to_string());
            }
            index += 1;
            continue;
        }
        if let Some((level, heading)) = atx_heading(line) {
            if (2..=3).contains(&level) {
                if heading.eq_ignore_ascii_case("transcript") {
                    active = BodySection::Transcript;
                } else if is_next_steps_heading(heading) {
                    notes.next_steps_heading = Some(heading.to_string());
                    notes.next_steps_level = level;
                    notes.content_order.push(ContentRef::NextSteps);
                    active = BodySection::NextSteps;
                } else {
                    let section_index = notes.sections.len();
                    notes.sections.push(MeetingSection {
                        heading: Some(heading.to_string()),
                        level,
                        items: Vec::new(),
                    });
                    notes
                        .content_order
                        .push(ContentRef::Section(section_index));
                    active = BodySection::Section(section_index);
                }
                index += 1;
                continue;
            }
        }

        if line.starts_with("- ") {
            let item = line[2..].to_string();
            match active {
                BodySection::NextSteps => {
                    notes.next_steps.push(parse_next_step(item))
                }
                BodySection::Section(section_index) => {
                    if let Some(section) = notes.sections.get_mut(section_index)
                    {
                        section.items.push(item);
                    }
                }
                BodySection::None => {
                    let section_index = unheaded_section(&mut notes);
                    notes.sections[section_index].items.push(item);
                    active = BodySection::Section(section_index);
                }
                BodySection::Transcript => {
                    unreachable!("transcript is handled above")
                }
            }
        } else if !line.trim().is_empty()
            && matches!(active, BodySection::Transcript)
        {
            transcript.push(line.to_string());
        }
        index += 1;
    }
    if !transcript.is_empty() {
        notes.transcript = Some(transcript.join("\n").trim().to_string());
    }
    notes
}

#[derive(Clone, Copy)]
enum BodySection {
    None,
    Section(usize),
    NextSteps,
    Transcript,
}

fn unheaded_section(notes: &mut MeetingNotes) -> usize {
    let index = notes.sections.len();
    notes.sections.push(MeetingSection {
        heading: None,
        level: 2,
        items: Vec::new(),
    });
    notes.content_order.push(ContentRef::Section(index));
    index
}

fn parse_next_step(text: String) -> MeetingNextStep {
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
        action: text,
    }
}

fn render_heading(level: u8, heading: &str, output: &mut String) {
    output.push_str(&"#".repeat(level as usize));
    output.push(' ');
    output.push_str(heading);
    output.push_str("\n\n");
}

fn tldr_fence(line: &str) -> Option<&str> {
    class_fence(line, "meeting-tldr")
}

fn class_fence(line: &str, class_name: &str) -> Option<&str> {
    let (fence, class) = line.trim().split_once(' ')?;
    if class != format!("{{.{class_name}}}")
        || fence.len() < 3
        || !fence.bytes().all(|byte| byte == b':')
    {
        return None;
    }
    Some(fence)
}

fn render_fenced_div(class_name: &str, content: &str, output: &mut String) {
    let fence_len = content
        .lines()
        .map(leading_colon_run)
        .max()
        .unwrap_or(0)
        .max(2)
        + 1;
    let fence = ":".repeat(fence_len);
    output.push_str(&fence);
    output.push_str(&format!(" {{.{class_name}}}\n"));
    output.push_str(content.trim());
    output.push('\n');
    output.push_str(&fence);
    output.push_str("\n\n");
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

fn is_next_steps_heading(heading: &str) -> bool {
    matches!(
        heading
            .trim()
            .trim_end_matches(':')
            .to_ascii_lowercase()
            .as_str(),
        "next steps" | "action items" | "follow-ups" | "follow ups"
    )
}

fn leading_colon_run(line: &str) -> usize {
    line.trim_start()
        .bytes()
        .take_while(|byte| *byte == b':')
        .count()
}
