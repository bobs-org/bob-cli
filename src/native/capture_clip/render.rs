use super::{ClipMode, ClipOutput};

pub(crate) fn is_valid_header(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

pub(crate) fn rendered_header(value: &str) -> String {
    value.to_ascii_uppercase().replace('_', " ")
}

pub(super) fn rendered_lines(
    header: Option<&str>,
    items: &[String],
    indent: &str,
) -> Vec<String> {
    if let Some(header) = header {
        if items.len() == 1 {
            return vec![format!("{indent}- **{header}:** {}", items[0])];
        }
        let mut lines = vec![format!("{indent}- **{header}:**")];
        lines.extend(
            items.iter().map(|item| format!("{indent}{indent}- {item}")),
        );
        return lines;
    }

    items
        .iter()
        .map(|item| format!("{indent}- {item}"))
        .collect()
}

pub(super) fn inline_output(
    header: Option<&str>,
    text: &str,
    indent: &str,
) -> ClipOutput {
    ClipOutput {
        header: header.map(str::to_string),
        mode: ClipMode::Inline,
        lines: rendered_lines(header, &[text.to_string()], indent),
        attachments: Vec::new(),
        snippet: None,
        entries: Vec::new(),
    }
}

pub(super) fn lines_output(
    header: Option<&str>,
    clipboard_lines: &[&str],
    indent: &str,
) -> ClipOutput {
    let items = clipboard_lines
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    ClipOutput {
        header: header.map(str::to_string),
        mode: ClipMode::Lines,
        lines: rendered_lines(header, &items, indent),
        attachments: Vec::new(),
        snippet: None,
        entries: Vec::new(),
    }
}
