//! Completion/cancel dates read off a task line.

/// The completion or cancel date on `line`, if any.
///
/// Accepts `[completion:: D]`, `[cancelled:: D]`, `✅ D`/`✔ D`, and
/// `❌ D`/`✖ D`.
pub(crate) fn close_date(line: &str) -> Option<String> {
    if let Some(date) = bracket_task_date(line, "completion") {
        return Some(date);
    }
    if let Some(date) = bracket_task_date(line, "cancelled") {
        return Some(date);
    }
    if let Some(date) = emoji_task_date(line, ['✅', '✔']) {
        return Some(date);
    }
    if let Some(date) = emoji_task_date(line, ['❌', '✖']) {
        return Some(date);
    }
    None
}

fn bracket_task_date(line: &str, key: &str) -> Option<String> {
    let mut rest = line;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else {
            break;
        };
        let inside = &after[..end];
        if let Some((name, value)) = inside.split_once("::")
            && name.trim().eq_ignore_ascii_case(key)
        {
            let date = value
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches([',', '.', ';'])
                .to_string();
            if !date.is_empty() {
                return Some(date);
            }
        }
        rest = &after[end + 1..];
    }
    None
}

fn emoji_task_date(line: &str, marks: [char; 2]) -> Option<String> {
    for mark in marks {
        if let Some(pos) = line.find(mark) {
            let after = line[pos + mark.len_utf8()..].trim_start();
            let date = after
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches([',', '.', ';']);
            if !date.is_empty() {
                return Some(date.to_string());
            }
        }
    }
    None
}
