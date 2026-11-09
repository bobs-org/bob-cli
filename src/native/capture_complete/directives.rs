//! Bob-authored completion rows for capture's `%` directives.

use std::{fs, path::Path};

use super::model::{CompleteError, DirectiveCandidate};
use crate::native::{
    capture::{
        select_default_pomodoro_line, DefaultPomodoroBranch,
        DefaultPomodoroSelection,
    },
    capture_language::{
        self, CompletionField, EditorItemParse, EditorMode, SpanKind,
    },
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    config, pomodoro,
    url_routing::{classify_token, RoutingEntry, UrlRoutingPolicy},
};

pub(super) fn candidates(
    bob_dir: &Path,
    raw_text: &str,
    field: &CompletionField,
    item: Option<&EditorItemParse>,
) -> Result<Vec<DirectiveCandidate>, CompleteError> {
    let template = capture_language::template::analyze(raw_text);
    let highlights =
        config::load_highlights_config(&config::config_path()).ok();
    let listening_configured = highlights
        .as_ref()
        .and_then(|config| config.listen_command())
        .is_some();
    let reference_policy =
        highlights.as_ref().map(UrlRoutingPolicy::from_highlights);
    let is_reference = item.is_some_and(|item| {
        item.mode == EditorMode::Ref
            || item.spans.iter().any(|span| span.kind == SpanKind::RefUrl)
            || reference_policy.as_ref().is_some_and(|policy| {
                bare_url_reference(raw_text, field, item, policy)
            })
    });
    let note_target = pomodoro_note_target(bob_dir);
    let query = field.query.to_ascii_lowercase();

    let mut rows = Vec::with_capacity(if is_reference { 4 } else { 3 });
    for (token, name, summary, detail) in [
        (
            "%c",
            "Clip",
            "Paste clipboard content",
            "Add a count, such as `%c3`, to paste the last three clips.",
        ),
        (
            "%n",
            "Pomodoro note",
            "Add a note to a Pomodoro",
            note_target.detail.as_str(),
        ),
        (
            "%t",
            "Template",
            "Expand this draft once per row",
            "Put a blank line after the first item, then add one row per capture.",
        ),
    ]
    .into_iter()
    .chain(is_reference.then_some((
        "%l",
        "Listen",
        "Queue this URL with companion audio",
        if listening_configured {
            "A listen command is configured."
        } else {
            "No `highlights.listen_command` is configured."
        },
    )))
    {
        if !query.is_empty()
            && !token[1..].to_ascii_lowercase().starts_with(&query)
        {
            continue;
        }

        let mut disabled_reason = match token {
            "%c" if template.directive.is_some() => Some(
                "A template can't capture the clipboard: `%c` would repeat once per row."
                    .to_string(),
            ),
            "%c" => None,
            "%n" => item.and_then(|item| {
                directive_refusal(
                    raw_text,
                    field,
                    item.index,
                    token,
                    reference_policy.as_ref(),
                )
            })
                .or_else(|| note_target.disabled.clone()),
            "%t" if item.is_some_and(|item| item.index > 0) => Some(
                "`%t` marks the first item as the template.".to_string(),
            ),
            "%t" => item.and_then(|item| {
                directive_refusal(
                    raw_text,
                    field,
                    item.index,
                    token,
                    reference_policy.as_ref(),
                )
            }),
            "%l" if !listening_configured => Some(
                "Configure `highlights.listen_command` to enable companion audio."
                    .to_string(),
            ),
            "%l" => item.and_then(|item| {
                directive_refusal(
                    raw_text,
                    field,
                    item.index,
                    token,
                    reference_policy.as_ref(),
                )
            }),
            _ => None,
        };

        // Replacing a completed directive with the same row is harmless;
        // duplicate-marker diagnostics are relevant only to a different
        // directive already present on the item.
        if token == "%n"
            && item.is_some_and(|item| {
                item.spans.iter().any(|span| {
                    span.kind == SpanKind::Directive
                        && raw_text
                            .get(span.start..span.end)
                            .is_some_and(|existing| existing == "%n")
                        && (span.start, span.end) != field.replacement
                })
            })
        {
            disabled_reason = Some(
                "A Pomodoro note can use only one `%n` directive per item."
                    .to_string(),
            );
        }

        let replacement = if disabled_reason.is_some() {
            String::new()
        } else {
            let mut replacement = token.to_string();
            if leading_marker(raw_text, field)
                && !raw_text
                    .get(field.replacement.1..)
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(char::is_whitespace)
            {
                replacement.push(' ');
            }
            replacement
        };
        let cursor_after = if replacement.is_empty() {
            field.replacement.0
        } else {
            field.replacement.0 + replacement.len()
        };
        rows.push(DirectiveCandidate {
            token: token.to_string(),
            name,
            summary,
            detail: Some(detail.to_string()),
            disabled_reason,
            replacement,
            cursor_after,
        });
    }
    Ok(rows)
}

fn bare_url_reference(
    raw_text: &str,
    field: &CompletionField,
    item: &EditorItemParse,
    policy: &UrlRoutingPolicy,
) -> bool {
    let Some(item_text) = raw_text.get(item.start..item.end) else {
        return false;
    };
    if item_text.lines().count() != 1 {
        return false;
    }
    let Some(line) = capture_language::split_physical_lines(raw_text)
        .into_iter()
        .find(|line| {
            field.replacement.0 >= line.start && field.replacement.1 <= line.end
        })
    else {
        return false;
    };
    let local_start = field.replacement.0 - line.start;
    let local_end = field.replacement.1 - line.start;
    let Some(mut text) = line.text.get(..).map(str::to_owned) else {
        return false;
    };
    if !text.is_char_boundary(local_start) || !text.is_char_boundary(local_end)
    {
        return false;
    }
    text.replace_range(
        local_start..local_end,
        &" ".repeat(local_end - local_start),
    );
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    let (url, route) = match tokens.as_slice() {
        [url] => (*url, None),
        [left, right] => {
            if classify_token(left).is_some() {
                (*left, Some(*right))
            } else if classify_token(right).is_some() {
                (*right, Some(*left))
            } else {
                return false;
            }
        }
        _ => return false,
    };
    if let Some(route) = route {
        let Some(route) = route.strip_prefix('@') else {
            return false;
        };
        if !capture_language::is_route_token(route) {
            return false;
        }
    }
    classify_token(url)
        .is_some_and(|intent| policy.admits(&intent, RoutingEntry::Capture))
}

fn directive_refusal(
    raw_text: &str,
    field: &CompletionField,
    item_index: usize,
    token: &str,
    url_routing: Option<&UrlRoutingPolicy>,
) -> Option<String> {
    let mut probe = raw_text.to_string();
    probe.replace_range(field.replacement.0..field.replacement.1, token);
    let parsed = capture_language::parse_for_editor_with(
        &probe,
        &capture_language::EditorParseOptions {
            url_routing,
            has_global_destination: false,
            // Record-aware completion rows belong to a later phase;
            // completion probes keep the literal fallback here.
            record_policy: None,
            notebook_policy: None,
        },
    );
    parsed
        .items
        .into_iter()
        .find(|item| item.index == item_index)?
        .diagnostics
        .into_iter()
        .find(|diagnostic| {
            matches!(
                diagnostic.code,
                "pomodoro_note_conflict"
                    | "template_directive_misplaced"
                    | "directive_not_applicable"
            )
        })
        .map(|diagnostic| diagnostic.message)
}

fn leading_marker(raw_text: &str, field: &CompletionField) -> bool {
    let Some(line) = capture_language::split_physical_lines(raw_text)
        .into_iter()
        .find(|line| {
            field.replacement.0 >= line.start && field.replacement.0 <= line.end
        })
    else {
        return false;
    };
    let Some(prefix_end) = field.replacement.0.checked_sub(line.start) else {
        return false;
    };
    let Some(prefix) = line.text.get(..prefix_end) else {
        return false;
    };
    let prefix = prefix.trim_start();
    let body_prefix = prefix
        .strip_prefix("- ")
        .or_else(|| prefix.strip_prefix("* "))
        .or_else(|| prefix.strip_prefix("+ "))
        .unwrap_or(prefix);
    body_prefix.trim().is_empty()
}

struct PomodoroNoteTarget {
    detail: String,
    disabled: Option<String>,
}

fn pomodoro_note_target(bob_dir: &Path) -> PomodoroNoteTarget {
    let day_file = pomodoro::day_file_for(bob_dir);
    let Ok(contents) = fs::read_to_string(&day_file) else {
        return no_pomodoro("No eligible Pomodoro in today's daily note.");
    };
    let pick = match select_default_pomodoro_line(
        &contents,
        DefaultPomodoroSelection::CurrentOrLastCompleted,
    ) {
        Ok(pick) => pick,
        Err(error) => {
            return no_pomodoro(map_pomodoro_note_error(&error.message))
        }
    };
    let scan = capture_pomodoros::scan(&contents);
    let entry = scan
        .entries
        .iter()
        .find(|entry| entry.line == pick.index + 1);
    let detail = if let Some(entry) = entry.filter(|entry| {
        entry.time_range.is_some()
            || entry.name.as_deref().is_some_and(|name| !name.is_empty())
    }) {
        format!("→ {}", pomodoro_target_label(entry))
    } else {
        let state = match pick.branch {
            DefaultPomodoroBranch::TimedOpen => "running",
            DefaultPomodoroBranch::LastCompleted => "last completed",
            DefaultPomodoroBranch::FirstOpen => "planned",
        };
        format!("→ {} · {state}", pick.task_text)
    };
    PomodoroNoteTarget {
        detail,
        disabled: None,
    }
}

fn map_pomodoro_note_error(message: &str) -> &str {
    match message {
        "Bob daily note has no Pomodoros section" => {
            "Today's daily note has no Pomodoros section."
        }
        "Bob daily note has multiple open timed Pomodoros" => {
            "Today's note has multiple open timed Pomodoros."
        }
        "Bob daily note has no eligible Pomodoro" => {
            "Today's daily note has no eligible Pomodoro."
        }
        other => other,
    }
}

fn no_pomodoro(reason: &str) -> PomodoroNoteTarget {
    PomodoroNoteTarget {
        detail: format!("No Pomodoro · {reason}"),
        disabled: Some(reason.to_string()),
    }
}

fn pomodoro_target_label(entry: &PomodoroEntry) -> String {
    let name = entry
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or("unnamed");
    let time = entry.time_range.as_deref().map(format_time_range);
    let state = if entry.is_current {
        "running"
    } else if entry.state == PomodoroState::Completed {
        "last completed"
    } else {
        "planned"
    };
    match time {
        Some(time) => format!("{time} {name} · {state}"),
        None => format!("{name} · {state}"),
    }
}

fn format_time_range(range: &str) -> String {
    let Some((start, end)) = range.split_once('-') else {
        return range.to_string();
    };
    if start.len() == 4
        && end.len() == 4
        && start.bytes().all(|byte| byte.is_ascii_digit())
        && end.bytes().all(|byte| byte.is_ascii_digit())
    {
        format!(
            "{}:{}–{}:{}",
            &start[..2],
            &start[2..],
            &end[..2],
            &end[2..]
        )
    } else {
        range.to_string()
    }
}
