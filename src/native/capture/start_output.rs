//! Human rendering for whole-item Pomodoro starts: the numbered queued
//! lineup, inline dropped rows, and the drop summary.
use super::*;

pub(super) fn print_human_pomodoro_start_success(
    result: &CaptureItemResult,
    start: &PomodoroStartSummary,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let verb = if result.dry_run {
        "would start"
    } else {
        "started"
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    let name = start
        .pomodoro_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or("next session");
    let created = if start.created_pomodoro {
        " (created)"
    } else {
        ""
    };
    println!(
        "  {}",
        styler.dim(&format!(
            "{} {}-{} ({}m){created} at line {}",
            name,
            start.start,
            start.end,
            start.duration_minutes,
            start.pomodoro_line,
        ))
    );
    println!("  {}", styler.dim(&result.task_line));
    // Queued-task lineup in the close's row style, with the unchanged
    // status marker instead of a transition. Link and task starts carry no
    // rows and print nothing here.
    let Some(tasks) = start.tasks.as_ref() else {
        return;
    };
    // Dropped rows print inline in index order, in the close's exact
    // wording, with the lane caption and then the nested-line caption.
    let mut dropped: Vec<&PomodoroStartTaskJson> =
        start.dropped.iter().collect();
    dropped.sort_by_key(|row| row.index);
    let width = tasks
        .iter()
        .chain(start.dropped.iter())
        .map(|row| row.index.to_string().len())
        .max()
        .unwrap_or(0);
    // Bold index with no outcome color; unnumbered output below stays
    // byte-identical to before when no row is numbered.
    let numbered = width > 0;
    let row_prefix = |index: u32| -> String {
        if !numbered {
            return "  ".to_string();
        }
        let digits = index.to_string().len();
        format!(
            "  {}{} ",
            " ".repeat(width.saturating_sub(digits)),
            styler.paint("1", &index.to_string())
        )
    };
    let mut kept = tasks.iter().peekable();
    let mut dropped = dropped.into_iter().peekable();
    loop {
        let next_kept = kept.peek().map(|row| row.index);
        let next_dropped = dropped.peek().map(|row| row.index);
        match (next_kept, next_dropped) {
            (Some(keep_at), Some(drop_at)) if drop_at < keep_at => {
                print_human_start_dropped_row(dropped.next().expect("peeked"));
            }
            (Some(_), Some(_)) => {
                print_human_start_kept_row(
                    kept.next().expect("peeked"),
                    &row_prefix,
                    styler,
                );
            }
            (Some(_), None) => {
                print_human_start_kept_row(
                    kept.next().expect("peeked"),
                    &row_prefix,
                    styler,
                );
            }
            (None, Some(_)) => {
                print_human_start_dropped_row(dropped.next().expect("peeked"));
            }
            (None, None) => break,
        }
    }
    if !start.dropped.is_empty() {
        let mut numbers: Vec<u32> =
            start.dropped.iter().map(|row| row.index).collect();
        numbers.sort_unstable();
        let summary = numbers
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {}", styler.dim(&format!("Dropped {summary}")));
    }
    if tasks.is_empty() {
        println!("  {}", styler.dim("nothing queued"));
    }
}

fn print_human_start_kept_row(
    task: &PomodoroStartTaskJson,
    row_prefix: &dyn Fn(u32) -> String,
    styler: &Styler,
) {
    let prefix = row_prefix(task.index);
    if !task.resolved {
        let warning = task.warning.as_deref().unwrap_or("unresolved target");
        println!("{prefix}{}", styler.dim(&format!("warning: {warning}")));
        return;
    }
    let marker = task
        .status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| "?".to_string());
    let text = task.text.as_deref().unwrap_or("");
    let locator = match &task.relative_target {
        Some(target) => format!("{target} ^{}", task.block_id),
        None => format!("^{}", task.block_id),
    };
    if text.is_empty() {
        println!("{prefix}{marker} {locator}");
    } else {
        println!("{prefix}{marker} {text} {locator}");
    }
}

fn print_human_start_dropped_row(task: &PomodoroStartTaskJson) {
    // The close's exact dropped wording: a dropped task keeps its lane, so
    // the row says which status it stays in. Unresolved dropped rows still
    // name their number and link (the `warning` lives in JSON).
    let mut line = format!("dropped {} {}", task.index, task.block_link);
    if let Some(status) = task.status_name.as_deref() {
        line.push_str(&format!(" · stays {status}"));
    }
    if task.nested_lines == 1 {
        line.push_str(" · +1 nested line");
    } else if task.nested_lines > 1 {
        line.push_str(&format!(" · +{} nested lines", task.nested_lines));
    }
    println!("  {line}");
}
