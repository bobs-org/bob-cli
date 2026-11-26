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
    AuthoredDepth, AuthoredSubBullet, CaptureItem, CaptureKind, DrawerSelector,
};
use super::place::{
    format_notebook_section_create_title, leftover_after, quoted_after_hash,
    scan_hash_opener, shift_range, token_end,
};
use crate::native::capture_drawers::{
    drawer_key, is_drawer_slug, normalize_drawer_literal,
};
use std::ops::Range;

/// A whole-bold authored container title and the UTF-8 ranges needed by
/// editor clients. The marker ranges cover the opening and closing `**`;
/// the title range excludes both delimiters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoldContainerParts {
    pub(crate) title: String,
    pub(crate) title_range: Range<usize>,
    pub(crate) marker_ranges: [Range<usize>; 2],
}

pub(crate) fn bold_container_parts(
    body: &str,
    base: usize,
) -> Option<BoldContainerParts> {
    use crate::native::capture_drawers::DrawerMarker;

    let (title, marker) =
        crate::native::capture_drawers::drawer_body_title(body)?;
    if marker != DrawerMarker::Bold {
        return None;
    }
    let title_start = title.as_ptr() as usize - body.as_ptr() as usize;
    let title_end = title_start + title.len();
    Some(BoldContainerParts {
        title: title.to_string(),
        title_range: base + title_start..base + title_end,
        marker_ranges: [
            base + title_start - 2..base + title_start,
            base + title_end..base + title_end + 2,
        ],
    })
}

pub(crate) fn empty_bold_container_error(title: &str) -> String {
    format!(
        "`- **{title}**` names a container but has no nested notes; nest them as `  - …`, or write `__{title}__` for a bold note"
    )
}

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
    let opener = scan_hash_opener(body)?;
    Some(shift_hit(
        parse_hit(body, opener.start, opener.sigil, opener.escaped),
        base,
    ))
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
    sub_bullets.iter().any(|item| item.depth.is_first())
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

/// Where a first-level bullet's drawer shortcut would land, derived from
/// the item's resolved shape. Completion reads this to decide whether the
/// bullet-start `#` opens a drawer menu and which drawers it offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DrawerBulletTarget {
    /// A note-less `@route+id` item: the existing task (or `^prj` note).
    ExistingTask { route: String, block_id: String },
    /// A new task: plain text, `@route`, `@route^id`, or `@route:id` with
    /// body text. `route` is `None` for the default inbox.
    NewTask { route: Option<String>, text: String },
    /// A new project note (`@route^id@`, `@route:id@`).
    NewProjectNote {
        route: String,
        block_id: String,
        text: String,
    },
}

/// Classify the under-task target of an item's first-level bullets from
/// the editor state *on entry* to drawer-bullet metadata (before a bare
/// `- #` clobbers the mode with `Incomplete`). `None` means a bullet-start
/// `#` here is not a drawer position.
pub(crate) fn classify_target(
    entry_mode: EditorMode,
    route: Option<&str>,
    block_id: Option<&str>,
    parent_body: &str,
    glued_section: Option<&str>,
    has_first_level_line: bool,
    explicit_toggle: bool,
) -> Option<DrawerBulletTarget> {
    match entry_mode {
        EditorMode::Task | EditorMode::PomodoroTask => {
            Some(DrawerBulletTarget::NewTask {
                route: route.map(str::to_string),
                text: parent_body.trim().to_string(),
            })
        }
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => {
            Some(DrawerBulletTarget::NewProjectNote {
                route: route?.to_string(),
                block_id: block_id?.to_string(),
                text: parent_body.trim().to_string(),
            })
        }
        EditorMode::SubBullet | EditorMode::TaskToggle => {
            let glued =
                glued_section.is_some_and(|section| !section.is_empty());
            if !parent_body.trim().is_empty()
                || glued
                || explicit_toggle
                || !has_first_level_line
            {
                return None;
            }
            Some(DrawerBulletTarget::ExistingTask {
                route: route?.to_string(),
                block_id: block_id?.to_string(),
            })
        }
        _ => None,
    }
}

/// Whether any physical child line of the item is a first-level authored
/// bullet, including a bare `- #` that parses into no sub-bullet yet.
pub(crate) fn item_has_first_level_line(item: &CaptureItem<'_>) -> bool {
    item.lines.iter().skip(1).any(|line| {
        matches!(
            super::draft::classify_authored_line(line.raw),
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth.is_first()
        )
    })
}

pub(crate) fn lexical_title_for_slug(slug: &str) -> String {
    format_notebook_section_create_title(slug)
}

pub(crate) fn drawer_bullet_text_error() -> String {
    "`- #notes` names a drawer, so it takes no text: nest \
`Postgres 17` beneath it (`- **Notes**` / `  - Postgres 17`); a task \
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
                normalize_task_text(&selector.text)
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
                "`- #notes` names a drawer, but these bullets nest \
under `{parent_body}`; file the note into it with \
`@foo+bar#notes {parent_body}`, or put the note on its own bullet"
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
            quoted_after_hash(section)
                .and_then(|quoted| normalize_drawer_literal(&quoted.decoded))
                .unwrap_or_else(|| normalize_task_text(section))
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
            "`- #notes` names a drawer, but these bullets nest \
under `{parent_body}`; file the note into it with \
`@foo+bar#notes {parent_body}`, or put the note on its own bullet"
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

pub(crate) fn code_for_message(message: &str) -> Option<&'static str> {
    if message == drawer_bullet_text_error() {
        Some("drawer_bullet_text")
    } else if message == empty_drawer_bullet_error()
        || message.starts_with("`- **")
            && message.contains("names a container but has no nested notes")
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

/// First-level groups: each first-level bullet plus every following
/// bullet below the first level, as `[start, end)` index ranges into
/// `sub_bullets`.
pub(crate) fn first_level_groups(
    sub_bullets: &[AuthoredSubBullet],
) -> Vec<(usize, usize)> {
    authored_level_groups(sub_bullets, AuthoredDepth::FIRST)
}

/// Groups at `level`: each bullet at exactly `level` plus every following
/// bullet deeper than it, as `[start, end)` index ranges into
/// `sub_bullets`. A shallower bullet, or a deeper one before the first
/// bullet at `level`, starts no group.
pub(crate) fn authored_level_groups(
    sub_bullets: &[AuthoredSubBullet],
    level: AuthoredDepth,
) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut index = 0usize;
    while index < sub_bullets.len() {
        if sub_bullets[index].depth != level {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < sub_bullets.len() && sub_bullets[index].depth > level {
            index += 1;
        }
        groups.push((start, index));
    }
    groups
}

/// Apply container-header grouping after `CaptureKind` is known: position
/// errors and childless-shortcut `empty_drawer_bullet`.
pub(crate) fn finish_execution_drawer_bullets(
    kind: &CaptureKind,
    parent_body: &str,
    sub_bullets: &mut [AuthoredSubBullet],
) -> Result<(), String> {
    let multi_note = is_multi_note_sub_bullet(kind, parent_body, sub_bullets);
    let under_task = is_under_task_kind(kind) || multi_note;
    let project_note_sections = matches!(kind, CaptureKind::ProjectNote { .. });
    let groups = first_level_groups(sub_bullets);
    for (start, end) in groups {
        let nested = end.saturating_sub(start) > 1;
        let body = &sub_bullets[start].body;
        if sub_bullets[start].drawer.is_some() {
            if !under_task {
                return Err(drawer_bullet_position_error(kind, parent_body));
            }
            if !nested {
                return Err(empty_drawer_bullet_error());
            }
            continue;
        }
        let Some(parts) = bold_container_parts(body, 0) else {
            continue;
        };
        let is_project_note = project_note_sections;
        let is_task_drawer = !is_project_note
            && matches!(
                kind,
                CaptureKind::Task
                    | CaptureKind::TaskWithBlockId { .. }
                    | CaptureKind::Pomodoro { close: None, .. }
                    | CaptureKind::SubBullet { .. }
            );
        if is_task_drawer
            && crate::native::capture_drawers::can_create_drawer(
                &parts.title,
                true,
            )
            .is_err()
        {
            // Reserved and otherwise ineligible whole-bold lines remain
            // ordinary notes. A leading `#` remains the explicit refusal
            // path for a forbidden selector.
            continue;
        }
        if !nested {
            return Err(empty_bold_container_error(&parts.title));
        }
        if is_task_drawer {
            sub_bullets[start].drawer = Some(DrawerSelector {
                text: parts.title,
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

fn parse_hit(
    line: &str,
    start: usize,
    sigil: usize,
    escaped: bool,
) -> DrawerBulletHit {
    let content_start = sigil + 1;
    let rest = &line[content_start..];
    let candidate_end = token_end(line, start);
    if let Some(quoted) = quoted_after_hash(rest) {
        let marker_end = content_start + quoted.literal_len;
        let remainder_end = if quoted.complete
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
        let raw_end = if quoted.complete {
            remainder_end
        } else {
            line.len()
        };
        if escaped {
            return DrawerBulletHit::Escaped {
                span: start..raw_end,
            };
        }
        if !quoted.complete || quoted.decoded.trim().is_empty() {
            return DrawerBulletHit::Incomplete {
                span: start..raw_end,
            };
        }
        let Some(title) = normalize_drawer_literal(&quoted.decoded) else {
            return DrawerBulletHit::Invalid {
                span: start..raw_end,
                raw: quoted.decoded,
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
        key: drawer_key(&title),
        leftover,
    }
}

fn shift_hit(hit: DrawerBulletHit, base: usize) -> DrawerBulletHit {
    let shift = |span: Range<usize>| shift_range(span, base);
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
