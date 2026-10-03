use std::path::Path;

use chrono::NaiveDateTime;

use super::{
    files::{
        classify_path_candidate, plan_attachments, plan_snippet, PathState,
    },
    model::FileReservations,
    render::{inline_output, lines_output},
    rendered_header, ClipMode, ClipOutput, ClipPlan, ClipReservations,
};

pub(super) const MAX_ATTACHMENT_COUNT: usize = 10;
pub(super) const MAX_INLINE_CHARACTERS: usize = 1000;
pub(super) const MAX_LINES: usize = 10;

#[cfg(test)]
pub(crate) fn plan(
    bob_dir: &Path,
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
) -> Result<ClipPlan, String> {
    let mut reservations = ClipReservations::default();
    plan_with_reservations(
        bob_dir,
        header,
        clipboard,
        now,
        indent,
        &mut reservations,
    )
}

pub(crate) fn plan_with_reservations(
    bob_dir: &Path,
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut ClipReservations,
) -> Result<ClipPlan, String> {
    let start = reservations.files.files.len();
    let output = plan_entry(
        bob_dir,
        header,
        clipboard,
        now,
        indent,
        &mut reservations.files,
    )?;
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
    plan_history_with_reservations(
        bob_dir,
        clipboards,
        now,
        indent,
        &mut reservations,
    )
}

pub(crate) fn plan_history_with_reservations(
    bob_dir: &Path,
    clipboards: &[String],
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut ClipReservations,
) -> Result<ClipPlan, String> {
    let start = reservations.files.files.len();
    let entries = clipboards
        .iter()
        .enumerate()
        .map(|(index, clipboard)| {
            plan_entry(
                bob_dir,
                None,
                clipboard,
                now,
                indent,
                &mut reservations.files,
            )
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
            header: None,
            mode: ClipMode::History,
            lines,
            attachments,
            snippet: None,
            entries,
        },
        files: reservations.files.files[start..].to_vec(),
    })
}

fn plan_entry(
    bob_dir: &Path,
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut FileReservations,
) -> Result<ClipOutput, String> {
    let header = header.map(rendered_header);
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
            header.as_deref(),
            path_states.iter().filter_map(|state| match state {
                PathState::File(path) => Some(path.as_path()),
                _ => None,
            }),
            indent,
            reservations,
        );
    }

    if let Some(items) = flat_unordered_list_items(&lines) {
        return Ok(lines_output(header.as_deref(), &items, indent));
    }

    if lines.len() == 1 {
        return if lines[0].chars().count() > MAX_INLINE_CHARACTERS {
            plan_snippet(
                bob_dir,
                header.as_deref(),
                clipboard,
                now,
                indent,
                reservations,
            )
        } else {
            Ok(inline_output(header.as_deref(), lines[0], indent))
        };
    }

    if lines.len() > MAX_LINES
        || lines.iter().any(|line| line.trim().is_empty())
        || lines.iter().any(|line| is_structural_line(line))
    {
        return plan_snippet(
            bob_dir,
            header.as_deref(),
            clipboard,
            now,
            indent,
            reservations,
        );
    }

    Ok(lines_output(header.as_deref(), &lines, indent))
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
