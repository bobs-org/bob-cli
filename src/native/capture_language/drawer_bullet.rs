//! Shared lexer for a first-level authored bullet's leading `#` drawer
//! shortcut.
//!
//! A `#` that begins a first-level bullet's body names a drawer. It is
//! never a type marker. Nested `  - #fix` still types. The opening
//! predicate matches today's type marker: an ASCII letter, `"` or `“`, or
//! a bare `#` followed by whitespace or end of line. `#` plus a digit,
//! `##`, and `#-` stay prose, and `\#` keeps a literal `#`.

use super::draft::strip_bullet_marker_at;
use super::editor_model::EditorMode;
use super::line::normalize_task_text;
use super::model::{
    AuthoredDepth, AuthoredSubBullet, CaptureKind, DrawerSelector,
};
use super::pomodoro_literal::lex_exact_name_after_hash;
use super::project_tasks::split_leading_checkbox;
use crate::native::capture_drawers::{
    drawer_key, is_drawer_slug, is_drawer_title, normalize_drawer_literal,
};
use std::ops::Range;

/// One leading drawer-shortcut classification on a first-level body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DrawerBulletHit {
    Complete {
        span: Range<usize>,
        selector: DrawerSelector,
        title: String,
        key: String,
        leftover: String,
    },
    Incomplete {
        span: Range<usize>,
    },
    Invalid {
        span: Range<usize>,
        raw: String,
    },
    Escaped {
        span: Range<usize>,
    },
}

impl DrawerBulletHit {
    pub(crate) fn span(&self) -> &Range<usize> {
        match self {
            Self::Complete { span, .. }
            | Self::Incomplete { span }
            | Self::Invalid { span, .. }
            | Self::Escaped { span } => span,
        }
    }
}

/// Scan the start of a first-level authored body (marker already stripped)
/// for a leading drawer shortcut. `base` is the absolute start of `body`.
pub(crate) fn scan_leading(body: &str, base: usize) -> Option<DrawerBulletHit> {
    let trimmed_start = body.len() - body.trim_start().len();
    let rest = &body[trimmed_start..];
    if rest.is_empty() {
        return None;
    }
    let escaped = rest.starts_with("\\#");
    let sigil_rel = if escaped { 1 } else { 0 };
    if !rest[sigil_rel..].starts_with('#') {
        return None;
    }
    let after_hash = &rest[sigil_rel + 1..];
    let next = after_hash.chars().next();
    let opens = next.is_some_and(|character| {
        character.is_ascii_alphabetic() || matches!(character, '"' | '“')
    });
    let bare = next.is_none() || next.is_some_and(char::is_whitespace);
    if !opens && !bare {
        return None;
    }
    let start = trimmed_start;
    let sigil = start + sigil_rel;
    Some(shift_hit(parse_hit(body, start, sigil, escaped), base))
}

/// Scan a whole physical first-level child line (`- `, `* `, or `+ `).
pub(crate) fn scan_first_level_line(
    line: &str,
    base: usize,
) -> Option<DrawerBulletHit> {
    let (body_start, body) = strip_bullet_marker_at(line, 0)?;
    scan_leading(body, base + body_start)
}

pub(crate) fn is_under_task_kind(kind: &CaptureKind) -> bool {
    matches!(
        kind,
        CaptureKind::Task
            | CaptureKind::TaskWithBlockId { .. }
            | CaptureKind::Pomodoro { close: None, .. }
            | CaptureKind::ProjectNote { .. }
    )
}

/// A note-less `@route+id` (or `@route+id#drawer`) item whose parent line
/// carries no note text but has first-level bullets files each bullet as
/// its own note. In that multi-note form the existing target is an
/// under-task position for drawer bullets, so `- #future-work` names a
/// drawer instead of reporting `drawer_bullet_position`. A same-line note
/// (`@route+id <note>` + bullets) and a glued drawer item stay
/// not-under-task.
pub(crate) fn is_multi_note_sub_bullet(
    kind: &CaptureKind,
    parent_body: &str,
    sub_bullets: &[AuthoredSubBullet],
) -> bool {
    if !parent_body.trim().is_empty() {
        return false;
    }
    let CaptureKind::SubBullet { section, .. } = kind else {
        return false;
    };
    if section.is_some() {
        return false;
    }
    sub_bullets
        .iter()
        .any(|item| item.depth == AuthoredDepth::First)
}

pub(crate) fn is_under_task_mode(mode: EditorMode) -> bool {
    matches!(
        mode,
        EditorMode::Task
            | EditorMode::PomodoroTask
            | EditorMode::ProjectNote
            | EditorMode::PomodoroProjectNote
    )
}

pub(crate) fn lexical_title_for_slug(slug: &str) -> String {
    normalize_task_text(&slug.replace('-', " ")).to_ascii_uppercase()
}

pub(crate) fn drawer_bullet_text_error() -> String {
    "`- #requirements` names a drawer, so it takes no text: nest \
`Postgres 17` beneath it (`- REQUIREMENTS` / `  - Postgres 17`); a task \
type goes at the end of a line (`… #fix`)"
        .to_string()
}

pub(crate) fn empty_drawer_bullet_error() -> String {
    "`- #future-work` has no notes beneath it; nest them as `  - …` \
(capture never creates an empty drawer)"
        .to_string()
}

pub(crate) fn incomplete_drawer_error() -> String {
    "`#` is incomplete: add a drawer (`#future-work`), or write `\\#` to \
keep a literal `#`"
        .to_string()
}

pub(crate) fn invalid_quoted_drawer_error(decoded: &str) -> String {
    format!("`{decoded}` is not a drawer name")
}

pub(crate) fn drawer_bullet_position_error(
    kind: &CaptureKind,
    parent_body: &str,
) -> String {
    match kind {
        CaptureKind::SubBullet {
            section: Some(selector),
            ..
        } => {
            let title = if selector.exact {
                normalize_task_text(&selector.text).to_ascii_uppercase()
            } else {
                lexical_title_for_slug(&selector.text)
            };
            format!(
                "this item already files every bullet into {title}; drop \
`#{}` from `@foo+bar` to use drawer bullets",
                selector.text
            )
        }
        CaptureKind::SubBullet { .. } if !parent_body.trim().is_empty() => {
            format!(
                "`- #requirements` names a drawer, but these bullets nest \
under `{parent_body}`; file the note into it with \
`@foo+bar#requirements {parent_body}`, or put the note on its own bullet"
            )
        }
        _ => "only bullets that land directly under a task can name a \
drawer; write `\\#` to keep a literal `#`"
            .to_string(),
    }
}

pub(crate) fn editor_drawer_bullet_position_error(
    mode: EditorMode,
    parent_body: &str,
    glued_section: Option<&str>,
) -> String {
    if let Some(section) = glued_section.filter(|value| !value.is_empty()) {
        let title = if section.starts_with('"') || section.starts_with('“') {
            lex_exact_name_after_hash(section, section.len())
                .and_then(|lex| normalize_drawer_literal(&lex.decoded))
                .unwrap_or_else(|| {
                    normalize_task_text(section).to_ascii_uppercase()
                })
        } else {
            lexical_title_for_slug(section)
        };
        return format!(
            "this item already files every bullet into {title}; drop \
`#{section}` from `@foo+bar` to use drawer bullets"
        );
    }
    if matches!(mode, EditorMode::SubBullet) && !parent_body.trim().is_empty() {
        return format!(
            "`- #requirements` names a drawer, but these bullets nest \
under `{parent_body}`; file the note into it with \
`@foo+bar#requirements {parent_body}`, or put the note on its own bullet"
        );
    }
    "only bullets that land directly under a task can name a drawer; \
write `\\#` to keep a literal `#`"
        .to_string()
}

pub(crate) fn multi_note_marker_error(
    marker: &str,
    example: &str,
    verb: &str,
) -> String {
    format!(
        "`{marker}` decorates a single note: put the note on the `@foo+bar` \
line to {verb} it (`@foo+bar Call Morgan {example}`)"
    )
}

pub(crate) fn multi_note_schedule_error(offset: u64) -> String {
    multi_note_marker_error(
        &format!("s:{offset}"),
        &format!("s:{offset}"),
        "schedule",
    )
}

pub(crate) fn multi_note_priority_error(level: u64) -> String {
    multi_note_marker_error(
        &format!("${level}"),
        &format!("${level}"),
        "prioritize",
    )
}

pub(crate) fn multi_note_clip_error() -> String {
    multi_note_marker_error("%c", "%c", "clip")
}

pub(crate) fn existing_drawer_empty_error(
    title: &str,
    block_id: &str,
) -> String {
    format!(
        "`{title}` is already a drawer on ^{block_id}; nest the notes to add \
beneath `- {title}`"
    )
}

pub(crate) fn code_for_message(message: &str) -> Option<&'static str> {
    if message == drawer_bullet_text_error() {
        Some("drawer_bullet_text")
    } else if message == empty_drawer_bullet_error()
        || message.contains("is already a drawer on ^")
            && message.contains("nest the notes to add beneath `- ")
    {
        Some("empty_drawer_bullet")
    } else if message
        .contains("decorates a single note: put the note on the `@foo+bar`")
    {
        Some("multi_note_marker")
    } else if message.contains("names a drawer, but these bullets nest")
        || message.contains("already files every bullet into")
        || message.contains(
            "only bullets that land directly under a task can name a drawer",
        )
    {
        Some("drawer_bullet_position")
    } else if message.ends_with("is not a drawer name") {
        Some("invalid_drawer")
    } else {
        None
    }
}

/// First-level groups: each first-level bullet plus the nested bullets
/// that follow it, as `[start, end)` index ranges into `sub_bullets`.
pub(crate) fn first_level_groups(
    sub_bullets: &[AuthoredSubBullet],
) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut index = 0usize;
    while index < sub_bullets.len() {
        if sub_bullets[index].depth != AuthoredDepth::First {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < sub_bullets.len()
            && sub_bullets[index].depth == AuthoredDepth::Nested
        {
            index += 1;
        }
        groups.push((start, index));
    }
    groups
}

/// Apply under-task grouping after `CaptureKind` is known: position
/// errors, childless-shortcut `empty_drawer_bullet`, and ALL-CAPS drawer
/// bullets with nested notes.
pub(crate) fn finish_execution_drawer_bullets(
    kind: &CaptureKind,
    parent_body: &str,
    sub_bullets: &mut [AuthoredSubBullet],
) -> Result<(), String> {
    let under_task = is_under_task_kind(kind)
        || is_multi_note_sub_bullet(kind, parent_body, sub_bullets);
    let groups = first_level_groups(sub_bullets);
    for (start, end) in groups {
        let nested = end.saturating_sub(start) > 1;
        if sub_bullets[start].drawer.is_some() {
            if !under_task {
                return Err(drawer_bullet_position_error(kind, parent_body));
            }
            if !nested {
                return Err(empty_drawer_bullet_error());
            }
            continue;
        }
        if under_task
            && nested
            && is_all_caps_drawer_title(&sub_bullets[start].body)
        {
            let title = sub_bullets[start].body.trim().to_string();
            sub_bullets[start].drawer = Some(DrawerSelector {
                text: title,
                exact: true,
            });
        }
    }
    Ok(())
}

pub(crate) fn unescape_escaped_body(body: &str) -> String {
    let leading = body.len() - body.trim_start().len();
    let rest = &body[leading..];
    if let Some(stripped) = rest.strip_prefix('\\') {
        format!("{}{stripped}", &body[..leading])
    } else {
        body.to_string()
    }
}

pub(crate) fn is_all_caps_drawer_title(body: &str) -> bool {
    let trimmed = body.trim();
    if split_leading_checkbox(trimmed).0.is_some() {
        return false;
    }
    is_drawer_title(trimmed)
}

fn parse_hit(
    line: &str,
    start: usize,
    sigil: usize,
    escaped: bool,
) -> DrawerBulletHit {
    let content_start = sigil + 1;
    let rest = &line[content_start..];
    let candidate_end = token_end(line, start);
    if rest.starts_with('"') || rest.starts_with('“') {
        let lex = lex_exact_name_after_hash(rest, rest.len())
            .expect("quote opener yields a literal");
        let marker_end = content_start + lex.literal_len;
        let remainder_end = if lex.complete
            && marker_end < line.len()
            && !line[marker_end..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            token_end(line, marker_end)
        } else {
            marker_end
        };
        let raw_end = if lex.complete {
            remainder_end
        } else {
            line.len()
        };
        if escaped {
            return DrawerBulletHit::Escaped {
                span: start..raw_end,
            };
        }
        if !lex.complete || lex.decoded.trim().is_empty() {
            return DrawerBulletHit::Incomplete {
                span: start..raw_end,
            };
        }
        let Some(title) = normalize_drawer_literal(&lex.decoded) else {
            return DrawerBulletHit::Invalid {
                span: start..raw_end,
                raw: lex.decoded,
            };
        };
        if marker_end != raw_end {
            return DrawerBulletHit::Invalid {
                span: start..raw_end,
                raw: line[start..raw_end].to_string(),
            };
        }
        let leftover = leftover_after(line, marker_end);
        let key = drawer_key(&title);
        return DrawerBulletHit::Complete {
            span: start..marker_end,
            selector: DrawerSelector {
                text: title.clone(),
                exact: true,
            },
            title,
            key,
            leftover,
        };
    }
    let suffix = &line[content_start..candidate_end];
    if escaped {
        return DrawerBulletHit::Escaped {
            span: start..candidate_end,
        };
    }
    if suffix.is_empty() {
        return DrawerBulletHit::Incomplete {
            span: start..candidate_end,
        };
    }
    if !is_drawer_slug(suffix) {
        return DrawerBulletHit::Invalid {
            span: start..candidate_end,
            raw: line[start..candidate_end].to_string(),
        };
    }
    let title = lexical_title_for_slug(suffix);
    let leftover = leftover_after(line, candidate_end);
    DrawerBulletHit::Complete {
        span: start..candidate_end,
        selector: DrawerSelector {
            text: suffix.to_string(),
            exact: false,
        },
        title: title.clone(),
        key: title,
        leftover,
    }
}

fn leftover_after(line: &str, end: usize) -> String {
    line.get(end..).unwrap_or("").trim().to_string()
}

fn shift_hit(hit: DrawerBulletHit, base: usize) -> DrawerBulletHit {
    let shift = |span: Range<usize>| (span.start + base)..(span.end + base);
    match hit {
        DrawerBulletHit::Complete {
            span,
            selector,
            title,
            key,
            leftover,
        } => DrawerBulletHit::Complete {
            span: shift(span),
            selector,
            title,
            key,
            leftover,
        },
        DrawerBulletHit::Incomplete { span } => {
            DrawerBulletHit::Incomplete { span: shift(span) }
        }
        DrawerBulletHit::Invalid { span, raw } => DrawerBulletHit::Invalid {
            span: shift(span),
            raw,
        },
        DrawerBulletHit::Escaped { span } => {
            DrawerBulletHit::Escaped { span: shift(span) }
        }
    }
}

fn token_end(line: &str, start: usize) -> usize {
    let mut end = start;
    while end < line.len()
        && !line[end..].chars().next().unwrap().is_whitespace()
    {
        end += line[end..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    end
}
