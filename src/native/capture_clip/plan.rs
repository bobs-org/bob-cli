use std::path::Path;

use chrono::NaiveDateTime;

use super::{
    files::{
        classify_path_candidate, plan_attachments, plan_snippet, PathState,
    },
    model::FileReservations,
    render::{inline_output, lines_output},
    AgreedClipboardHistory, ClipHistoryOutput, ClipMode, ClipOutput, ClipPlan,
    ClipReservations, ClipStatus,
};

pub(super) const MAX_ATTACHMENT_COUNT: usize = 10;
pub(super) const MAX_INLINE_CHARACTERS: usize = 1000;
pub(super) const MAX_LINES: usize = 10;

/// One `bob attachments import` failure: the journal-code the CLI prints
/// plus the human message behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExplicitPathError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

/// Plan explicit paths through the same `classify_path_candidate` →
/// `plan_attachments` path `%c` uses, with fresh reservations and an
/// empty indent. The caller saves with `ClipPlan::save`, which never
/// edits notes.
pub(crate) fn plan_explicit_paths(
    bob_dir: &Path,
    raws: &[String],
) -> Result<ClipPlan, ExplicitPathError> {
    if raws.len() > MAX_ATTACHMENT_COUNT {
        return Err(ExplicitPathError {
            code: "too_many",
            message: format!(
                "cannot import {} paths; at most {MAX_ATTACHMENT_COUNT} are supported",
                raws.len()
            ),
        });
    }
    let mut sources = Vec::with_capacity(raws.len());
    for raw in raws {
        match classify_path_candidate(raw) {
            Ok(PathState::File(path)) => sources.push(path),
            Ok(PathState::Missing(path)) => {
                return Err(ExplicitPathError {
                    code: "missing",
                    message: format!(
                        "attachment does not exist: {}",
                        path.display()
                    ),
                });
            }
            Ok(PathState::Directory) => {
                return Err(ExplicitPathError {
                    code: "not_a_file",
                    message: format!("attachment is not a file: {raw}"),
                });
            }
            Ok(PathState::NotPath) => {
                return Err(ExplicitPathError {
                    code: "not_absolute",
                    message: format!(
                        "attachment is not an absolute path: {raw}"
                    ),
                });
            }
            Err(message) => {
                return Err(ExplicitPathError {
                    code: "io",
                    message,
                });
            }
        }
    }
    let mut reservations = ClipReservations::default();
    plan_attachments(
        bob_dir,
        sources.iter().map(|path| path.as_path()),
        "",
        &mut reservations.files,
    )
    .map(|output| ClipPlan {
        files: std::mem::take(&mut reservations.files.files),
        output,
    })
    .map_err(|message| ExplicitPathError {
        code: "io",
        message,
    })
}

#[cfg(test)]
pub(crate) fn plan(
    bob_dir: &Path,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
) -> Result<ClipPlan, String> {
    let mut reservations = ClipReservations::default();
    plan_with_reservations(bob_dir, clipboard, now, indent, &mut reservations)
}

pub(crate) fn plan_with_reservations(
    bob_dir: &Path,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut ClipReservations,
) -> Result<ClipPlan, String> {
    let start = reservations.files.files.len();
    let output =
        plan_entry(bob_dir, clipboard, now, indent, &mut reservations.files)?;
    Ok(ClipPlan {
        output,
        files: reservations.files.files[start..].to_vec(),
    })
}

#[cfg(test)]
pub(crate) fn plan_history(
    bob_dir: &Path,
    clipboards: &[String],
    now: NaiveDateTime,
    indent: &str,
) -> Result<ClipPlan, String> {
    let mut reservations = ClipReservations::default();
    plan_history_values_with_reservations(
        bob_dir,
        clipboards,
        now,
        indent,
        &mut reservations,
        None,
    )
}

pub(crate) fn plan_history_with_reservations(
    bob_dir: &Path,
    history: &AgreedClipboardHistory,
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut ClipReservations,
) -> Result<ClipPlan, String> {
    plan_history_values_with_reservations(
        bob_dir,
        &history.values,
        now,
        indent,
        reservations,
        Some(ClipHistoryOutput::from_rows(
            history.provider,
            &history.entries,
            std::time::SystemTime::now(),
        )),
    )
}

fn plan_history_values_with_reservations(
    bob_dir: &Path,
    clipboards: &[String],
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut ClipReservations,
    history: Option<ClipHistoryOutput>,
) -> Result<ClipPlan, String> {
    let start = reservations.files.files.len();
    let entries = clipboards
        .iter()
        .enumerate()
        .map(|(index, clipboard)| {
            plan_entry(bob_dir, clipboard, now, indent, &mut reservations.files)
                .map_err(|error| {
                    format!("clipboard history entry {}: {error}", index + 1)
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let lines = entries
        .iter()
        .flat_map(|entry| entry.lines.iter().cloned())
        .collect();
    let attachments = entries
        .iter()
        .flat_map(|entry| entry.attachments.iter().cloned())
        .collect();
    Ok(ClipPlan {
        output: ClipOutput {
            status: ClipStatus::Read,
            count: None,
            mode: ClipMode::History,
            lines,
            attachments,
            snippet: None,
            entries,
            history,
        },
        files: reservations.files.files[start..].to_vec(),
    })
}

fn plan_entry(
    bob_dir: &Path,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut FileReservations,
) -> Result<ClipOutput, String> {
    let lines = clipboard.split('\n').collect::<Vec<_>>();
    let path_states = lines
        .iter()
        .map(|line| classify_path_candidate(line))
        .collect::<Result<Vec<_>, _>>()?;

    for state in &path_states {
        if let PathState::Missing(path) = state {
            return Err(format!(
                "clipboard attachment does not exist: {}",
                path.display()
            ));
        }
    }

    let all_nonempty_attachments =
        lines.iter().zip(&path_states).all(|(line, state)| {
            !line.trim().is_empty() && matches!(state, PathState::File(_))
        });
    if all_nonempty_attachments {
        if lines.len() > MAX_ATTACHMENT_COUNT {
            return Err(format!(
                "clipboard contains {} attachments; at most {MAX_ATTACHMENT_COUNT} are supported",
                lines.len()
            ));
        }
        return plan_attachments(
            bob_dir,
            path_states.iter().filter_map(|state| match state {
                PathState::File(path) => Some(path.as_path()),
                _ => None,
            }),
            indent,
            reservations,
        );
    }

    if let Some(items) = flat_unordered_list_items(&lines) {
        return Ok(lines_output(&items, indent));
    }

    if lines.len() == 1 {
        return if lines[0].chars().count() > MAX_INLINE_CHARACTERS {
            plan_snippet(bob_dir, clipboard, now, indent, reservations)
        } else {
            Ok(inline_output(lines[0], indent))
        };
    }

    if lines.len() > MAX_LINES
        || lines.iter().any(|line| line.trim().is_empty())
        || lines.iter().any(|line| is_structural_line(line))
    {
        return plan_snippet(bob_dir, clipboard, now, indent, reservations);
    }

    Ok(lines_output(&lines, indent))
}

fn flat_unordered_list_items<'a>(lines: &[&'a str]) -> Option<Vec<&'a str>> {
    if lines.is_empty()
        || lines.len() > MAX_LINES
        || (lines.len() == 1
            && lines[0].chars().count() > MAX_INLINE_CHARACTERS)
    {
        return None;
    }

    lines
        .iter()
        .map(|line| {
            let rest = line
                .strip_prefix('-')
                .or_else(|| line.strip_prefix('*'))
                .or_else(|| line.strip_prefix('+'))?;
            if !rest.starts_with([' ', '\t']) {
                return None;
            }
            let item = rest.trim_start_matches([' ', '\t']);
            (!item.trim().is_empty()).then_some(item)
        })
        .collect()
}

pub(super) fn is_structural_line(line: &str) -> bool {
    if line.starts_with(' ') || line.starts_with('\t') {
        return true;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('#')
        || trimmed.starts_with("> ")
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
        || ["- ", "* ", "+ "]
            .iter()
            .any(|marker| trimmed.starts_with(marker))
    {
        return true;
    }
    let digit_count = trimmed
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digit_count > 0 && trimmed[digit_count..].starts_with(". ")
}
