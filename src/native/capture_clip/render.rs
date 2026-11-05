use super::{ClipMode, ClipOutput, ClipStatus};

pub(super) fn rendered_lines(items: &[String], indent: &str) -> Vec<String> {
    items
        .iter()
        .map(|item| format!("{indent}- {item}"))
        .collect()
}

pub(super) fn inline_output(text: &str, indent: &str) -> ClipOutput {
    ClipOutput {
        status: ClipStatus::Read,
        count: None,
        mode: ClipMode::Inline,
        lines: rendered_lines(&[text.to_string()], indent),
        attachments: Vec::new(),
        snippet: None,
        entries: Vec::new(),
        history: None,
    }
}

pub(super) fn lines_output(
    clipboard_lines: &[&str],
    indent: &str,
) -> ClipOutput {
    let items = clipboard_lines
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    ClipOutput {
        status: ClipStatus::Read,
        count: None,
        mode: ClipMode::Lines,
        lines: rendered_lines(&items, indent),
        attachments: Vec::new(),
        snippet: None,
        entries: Vec::new(),
        history: None,
    }
}
