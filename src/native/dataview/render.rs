//! Native markdown rendering for query results.

use super::*;

pub(super) fn markdown_table(
    headers: &[String],
    values: &[Vec<DataviewValue>],
    settings: &NativeMarkdownSettings,
) -> String {
    let mut rendered_rows = Vec::new();
    let mut max_lengths = headers
        .iter()
        .map(|header| escape_table(header).len())
        .collect::<Vec<_>>();

    for row in values {
        let rendered = (0..headers.len())
            .map(|index| {
                row.get(index)
                    .map(|value| {
                        escape_table(&markdown_table_literal(value, settings))
                    })
                    .unwrap_or_else(|| escape_table(&settings.render_null_as))
            })
            .collect::<Vec<_>>();
        for (index, cell) in rendered.iter().enumerate() {
            max_lengths[index] = max_lengths[index].max(cell.len());
        }
        rendered_rows.push(rendered);
    }

    let mut table = String::new();
    table.push_str("| ");
    table.push_str(
        &headers
            .iter()
            .enumerate()
            .map(|(index, header)| {
                padright(&escape_table(header), max_lengths[index])
            })
            .collect::<Vec<_>>()
            .join(" | "),
    );
    table.push_str(" |\n| ");
    table.push_str(
        &max_lengths
            .iter()
            .map(|length| "-".repeat(*length))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    table.push_str(" |\n");

    for row in rendered_rows {
        table.push_str("| ");
        table.push_str(
            &row.iter()
                .enumerate()
                .map(|(index, cell)| padright(cell, max_lengths[index]))
                .collect::<Vec<_>>()
                .join(" | "),
        );
        table.push_str(" |\n");
    }

    table
}

pub(super) fn markdown_table_literal(
    value: &DataviewValue,
    settings: &NativeMarkdownSettings,
) -> String {
    match value {
        DataviewValue::Array(values) => values
            .iter()
            .map(|value| markdown_literal(value, settings))
            .collect::<Vec<_>>()
            .join(", "),
        DataviewValue::Object(values) => values
            .iter()
            .map(|(key, value)| {
                format!("{key}: {}", markdown_literal(value, settings))
            })
            .collect::<Vec<_>>()
            .join(", "),
        value => markdown_literal(value, settings),
    }
}

pub(super) fn markdown_task_values(
    values: &[DataviewValue],
    settings: &NativeMarkdownSettings,
    depth: usize,
) -> String {
    if !values.is_empty()
        && values.iter().all(|value| task_group_value(value).is_some())
    {
        let mut markdown = String::new();
        for value in values {
            let Some((key, rows)) = task_group_value(value) else {
                continue;
            };
            markdown.push_str(&"#".repeat(depth + 1));
            markdown.push(' ');
            markdown.push_str(&markdown_literal(key, settings));
            markdown.push_str("\n\n");
            markdown.push_str(&markdown_task_values(rows, settings, depth + 1));
        }
        return markdown;
    }

    let mut markdown = String::new();
    for value in values {
        markdown.push_str(&markdown_task_value(value, settings, depth));
    }
    markdown
}

pub(super) fn task_group_value(
    value: &DataviewValue,
) -> Option<(&DataviewValue, &[DataviewValue])> {
    let key = value.as_object_field("key")?;
    let rows = match value.as_object_field("rows")? {
        DataviewValue::Array(rows) => rows.as_slice(),
        _ => return None,
    };
    Some((key, rows))
}

pub(super) fn markdown_task_value(
    value: &DataviewValue,
    settings: &NativeMarkdownSettings,
    depth: usize,
) -> String {
    let indent = "  ".repeat(depth);
    let task = value
        .as_object_field("task")
        .and_then(|value| match value {
            DataviewValue::Bool(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(false);
    let status = value
        .as_object_field("status")
        .and_then(DataviewValue::as_str)
        .and_then(|value| value.chars().next())
        .unwrap_or(' ');
    let text = value
        .as_object_field("visual")
        .or_else(|| value.as_object_field("text"))
        .and_then(DataviewValue::as_str)
        .map(|value| value.split('\n').collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| markdown_literal(value, settings));

    let mut markdown = String::new();
    markdown.push_str(&indent);
    markdown.push_str("- ");
    if task {
        markdown.push('[');
        markdown.push(status);
        markdown.push_str("] ");
    }
    markdown.push_str(&text);
    markdown.push('\n');

    if let Some(DataviewValue::Array(children)) =
        value.as_object_field("children")
    {
        markdown.push_str(&markdown_task_values(
            children.as_slice(),
            settings,
            depth + 1,
        ));
    }

    markdown
}

pub(super) fn markdown_literal(
    value: &DataviewValue,
    settings: &NativeMarkdownSettings,
) -> String {
    match value {
        DataviewValue::Null => settings.render_null_as.clone(),
        DataviewValue::Bool(value) => value.to_string(),
        DataviewValue::Number(value) => value.to_string(),
        DataviewValue::String(value)
        | DataviewValue::Date(value)
        | DataviewValue::DateTime(value)
        | DataviewValue::Duration(value) => value.clone(),
        DataviewValue::Link(link) => markdown_link(link),
        DataviewValue::Array(values) => values
            .iter()
            .map(|value| markdown_literal(value, settings))
            .collect::<Vec<_>>()
            .join(", "),
        DataviewValue::Object(values) => {
            let fields = values
                .iter()
                .map(|(key, value)| {
                    format!("{key}: {}", markdown_literal(value, settings))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {fields} }}")
        }
    }
}

pub(super) fn markdown_link(link: &DataviewLink) -> String {
    let mut markdown = String::new();
    if link.embed {
        markdown.push('!');
    }
    markdown.push_str("[[");
    markdown.push_str(&link.path.replace('|', "\\|"));
    markdown.push('|');
    let display = link
        .display
        .clone()
        .unwrap_or_else(|| default_link_display(&link.path));
    markdown.push_str(&display);
    markdown.push_str("]]");
    markdown
}

pub(super) fn default_link_display(path: &str) -> String {
    let (base, subpath) = path
        .split_once('#')
        .map_or((path, None), |(base, subpath)| {
            (base, Some(subpath.trim_start_matches('^')))
        });
    let mut display = note_stem(base).unwrap_or_else(|| base.to_string());
    if let Some(subpath) = subpath
        && !subpath.is_empty()
    {
        display.push_str(" > ");
        display.push_str(subpath);
    }
    display
}

pub(super) fn escape_table(text: &str) -> String {
    let mut output = String::new();
    let mut previous = None;
    for ch in text.chars() {
        if ch == '|' && previous != Some('\\') {
            output.push('\\');
        }
        output.push(ch);
        previous = Some(ch);
    }
    output
}

pub(super) fn padright(text: &str, length: usize) -> String {
    if text.len() >= length {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(length - text.len()))
    }
}
