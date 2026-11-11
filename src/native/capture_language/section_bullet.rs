//! Shared lexer for a first-level authored bullet's leading `#` notebook
//! section shortcut, plus the date-independent notebook body plan.
//!
//! A `#` that begins a first-level bullet in a notebook body (a route-only
//! `@nb` list or a `@parent^id@ %n` item) names a `##` section. It is never
//! a type marker and never a drawer. Nested one-line notes file into that
//! heading; the selector line is never a note. The opener matches today's
//! type/drawer predicate: an ASCII letter, `"` or `“`, or a bare `#`
//! followed by whitespace or end of line. `#` plus a digit, `##`, and `#-`
//! stay prose, and `\#` keeps a literal `#`.

use super::draft::classify_authored_line;
#[cfg(test)]
use super::draft::strip_bullet_marker_at;
use super::editor_model::{Diagnostic, Need, Severity};
use super::line::normalize_task_text;
use super::model::{AuthoredDepth, AuthoredSubBullet, CaptureItem};
use super::notebook::{
    resolve_notebook_section, strip_trailing_caret_id, NotebookSection,
    NotebookSectionResolution,
};
use super::pomodoro_literal::lex_exact_name_after_hash;
use crate::native::capture_project_note::{
    format_project_section_title, is_project_section_title,
};
use std::ops::Range;

/// One leading section-shortcut classification on a first-level body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SectionBulletHit {
    Complete {
        span: Range<usize>,
        selector: String,
        exact: bool,
        leftover: String,
        authored: String,
    },
    Incomplete {
        span: Range<usize>,
    },
    Escaped {
        span: Range<usize>,
    },
}

/// Scan the start of a first-level authored body (marker already stripped)
/// for a leading section shortcut. `base` is the absolute start of `body`.
pub(crate) fn scan_leading(
    body: &str,
    base: usize,
) -> Option<SectionBulletHit> {
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
#[cfg(test)]
pub(crate) fn scan_first_level_line(
    line: &str,
    base: usize,
) -> Option<SectionBulletHit> {
    let (body_start, body) = strip_bullet_marker_at(line, 0)?;
    scan_leading(body, base + body_start)
}

pub(crate) fn unescape_escaped_body(body: &str) -> String {
    super::drawer_bullet::unescape_escaped_body(body)
}

pub(crate) fn section_bullet_text_error() -> String {
    "`- #git` names a section, so it takes no text: nest notes beneath it \
(`  - …`), or write `\\#git` to keep the tag"
        .to_string()
}

pub(crate) fn empty_section_bullet_error(authored: &str) -> String {
    format!(
        "`- {authored}` has no notes beneath it; nest them as `  - …` \
(capture never creates an empty section)"
    )
}

pub(crate) fn section_bullet_position_error(
    title: &str,
    authored: &str,
    route: &str,
    header: &str,
) -> String {
    format!(
        "this list already files every bullet into ## {title}; drop \
`{authored}` from `@{route}#{header}` to use section bullets"
    )
}

pub(crate) fn section_bullet_depth_error(
    route: &str,
    selector: &str,
) -> String {
    let slug = if selector.is_empty() {
        "git".to_string()
    } else {
        super::line::selector_slug(selector)
    };
    let slug = if slug.is_empty() {
        "git"
    } else {
        slug.as_str()
    };
    format!(
        "notes in a section bullet are one line; for a note with detail, \
use a `@{route}#{slug}` header list"
    )
}

pub(crate) fn notebook_section_required_error() -> String {
    "a notebook section marker needs a section name".to_string()
}

pub(crate) fn looks_like_deeper_bullet(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let indent = line.len() - trimmed.len();
    if indent <= 2 {
        return false;
    }
    matches!(trimmed.as_bytes().first(), Some(b'-' | b'*' | b'+'))
        && trimmed
            .as_bytes()
            .get(1)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
}

/// One note collected by the shared body plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookPlannedNote {
    pub(crate) text: String,
    pub(crate) block_id: Option<String>,
    pub(crate) detail: Vec<NotebookPlannedNote>,
    pub(crate) source_range: Range<usize>,
    pub(crate) line_number: usize,
    pub(crate) escaped: bool,
}

/// One section group collected by the shared body plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookPlannedSection {
    pub(crate) selector: String,
    pub(crate) exact: bool,
    pub(crate) authored_marker: String,
    pub(crate) selector_span: Range<usize>,
    pub(crate) line_number: usize,
    pub(crate) notes: Vec<NotebookPlannedNote>,
    pub(crate) all_caps: bool,
}

/// Date-independent seed/body: top notes in source order, then section
/// groups in first-mention order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct NotebookBodyPlan {
    pub(crate) top_notes: Vec<NotebookPlannedNote>,
    pub(crate) sections: Vec<NotebookPlannedSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotebookBodyOutcome {
    Ready(NotebookBodyPlan),
    Pending {
        plan: NotebookBodyPlan,
        needs: Vec<Need>,
        placeholder: Option<Range<usize>>,
        diagnostics: Vec<Diagnostic>,
    },
    Failed {
        diagnostics: Vec<Diagnostic>,
        needs: Vec<Need>,
        placeholder: Option<Range<usize>>,
        plan: NotebookBodyPlan,
    },
}

impl NotebookBodyOutcome {
    fn failed(diagnostic: Diagnostic) -> Self {
        Self::Failed {
            diagnostics: vec![diagnostic],
            needs: Vec::new(),
            placeholder: None,
            plan: NotebookBodyPlan::default(),
        }
    }
}

/// Plan one list-item notebook body from the item's physical lines.
pub(crate) fn plan_list_item(
    item: &CaptureItem<'_>,
    route: &str,
) -> NotebookBodyOutcome {
    plan_list_lines(item, route)
}

/// Plan a `%n` create body from the item's physical lines.
pub(crate) fn plan_create_item(
    item: &CaptureItem<'_>,
    route: &str,
) -> NotebookBodyOutcome {
    plan_create_lines(item, route)
}

/// Reconstruct a complete plan from already-claimed sub-bullets. Used by
/// the `%n` writer, which only adds dates. Offsets are unused there.
pub(crate) fn plan_from_sub_bullets(
    sub_bullets: &[AuthoredSubBullet],
    all_caps_as_section: bool,
) -> NotebookBodyPlan {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < sub_bullets.len() {
        let bullet = &sub_bullets[index];
        if bullet.depth != AuthoredDepth::First {
            index += 1;
            continue;
        }
        let nested_start = index + 1;
        index += 1;
        while index < sub_bullets.len()
            && sub_bullets[index].depth == AuthoredDepth::Nested
        {
            index += 1;
        }
        let children = &sub_bullets[nested_start..index];
        let notes = children
            .iter()
            .map(|child| note_from_body(&child.body, 0..0, 0, false))
            .collect();
        groups.push(group_from_first_level(
            &bullet.body,
            0,
            0..bullet.body.len(),
            0,
            notes,
            all_caps_as_section,
        ));
    }
    merge_groups(groups, &[])
}

fn plan_list_lines(item: &CaptureItem<'_>, route: &str) -> NotebookBodyOutcome {
    let Some(first) = item.lines.first() else {
        return NotebookBodyOutcome::Ready(NotebookBodyPlan::default());
    };
    let body = first.raw.text;
    let hit = scan_leading(body, first.raw.start);
    let section_group = match &hit {
        Some(SectionBulletHit::Complete { leftover, .. })
            if leftover.is_empty() =>
        {
            true
        }
        Some(SectionBulletHit::Incomplete { .. }) => true,
        _ => false,
    };
    let nested = match collect_nested_lines(item, 1, route, section_group) {
        Ok(nested) => nested,
        Err(outcome) => return outcome,
    };
    match hit {
        Some(SectionBulletHit::Incomplete { span }) => pending_section(span),
        Some(SectionBulletHit::Complete { span, leftover, .. })
            if !leftover.is_empty() =>
        {
            NotebookBodyOutcome::failed(error_at(
                "section_bullet_text",
                section_bullet_text_error(),
                Some((span.start, span.end)),
            ))
        }
        Some(SectionBulletHit::Complete {
            span,
            selector,
            exact,
            authored,
            leftover: _,
        }) => {
            if nested.notes.is_empty() {
                return NotebookBodyOutcome::Pending {
                    plan: NotebookBodyPlan::default(),
                    needs: vec![Need::SectionNotes],
                    placeholder: None,
                    diagnostics: Vec::new(),
                };
            }
            NotebookBodyOutcome::Ready(NotebookBodyPlan {
                top_notes: Vec::new(),
                sections: vec![NotebookPlannedSection {
                    selector,
                    exact,
                    authored_marker: authored,
                    selector_span: span,
                    line_number: first.line_number,
                    notes: nested.notes,
                    all_caps: false,
                }],
            })
        }
        Some(SectionBulletHit::Escaped { .. }) => {
            let text = unescape_escaped_body(body);
            let mut top = note_from_body(
                &text,
                first.raw.start..first.raw.end,
                first.line_number,
                true,
            );
            top.detail = nested.notes;
            NotebookBodyOutcome::Ready(NotebookBodyPlan {
                top_notes: vec![top],
                sections: Vec::new(),
            })
        }
        None => {
            let mut top = note_from_body(
                body,
                first.raw.start..first.raw.end,
                first.line_number,
                false,
            );
            top.detail = nested.notes;
            NotebookBodyOutcome::Ready(NotebookBodyPlan {
                top_notes: vec![top],
                sections: Vec::new(),
            })
        }
    }
}

fn pending_section(span: Range<usize>) -> NotebookBodyOutcome {
    NotebookBodyOutcome::Pending {
        plan: NotebookBodyPlan::default(),
        needs: vec![Need::Section],
        placeholder: Some(span.clone()),
        diagnostics: Vec::new(),
    }
}

fn pending_section_notes() -> NotebookBodyOutcome {
    NotebookBodyOutcome::Pending {
        plan: NotebookBodyPlan::default(),
        needs: vec![Need::SectionNotes],
        placeholder: None,
        diagnostics: Vec::new(),
    }
}

fn plan_create_lines(
    item: &CaptureItem<'_>,
    route: &str,
) -> NotebookBodyOutcome {
    let mut groups = Vec::new();
    let mut pending: Option<NotebookBodyOutcome> = None;
    let mut index = 1;
    while index < item.lines.len() {
        let line = item.lines[index];
        match classify_authored_line(line.raw) {
            super::draft::AuthoredLineClass::EmptyOrPlaceholder => {
                index += 1;
            }
            super::draft::AuthoredLineClass::Invalid => {
                if looks_like_deeper_bullet(line.raw.text)
                    && matches!(groups.last(), Some(PlannedGroup::Section(_)))
                {
                    let selector = match groups.last() {
                        Some(PlannedGroup::Section(section)) => {
                            section.selector.as_str()
                        }
                        _ => "",
                    };
                    return NotebookBodyOutcome::failed(error_at(
                        "section_bullet_depth",
                        section_bullet_depth_error(route, selector),
                        Some((line.raw.start, line.raw.end)),
                    ));
                }
                return NotebookBodyOutcome::failed(error_at(
                    "invalid_child_line",
                    super::line::invalid_child_line_error(line.line_number),
                    Some((line.raw.start, line.raw.end)),
                ));
            }
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == AuthoredDepth::First =>
            {
                let hit = scan_leading(authored.body, authored.body_start);
                let nested_follows =
                    item.lines.get(index + 1).is_some_and(|next| {
                        matches!(
                            classify_authored_line(next.raw),
                            super::draft::AuthoredLineClass::Item(child)
                                if child.depth == AuthoredDepth::Nested
                        )
                    });
                let section_group =
                    match &hit {
                        Some(SectionBulletHit::Complete {
                            leftover, ..
                        }) if leftover.is_empty() => true,
                        Some(SectionBulletHit::Incomplete { .. }) => true,
                        None if is_project_section_title(authored.body)
                            && nested_follows =>
                        {
                            true
                        }
                        _ => false,
                    };
                let nested = match collect_nested_lines(
                    item,
                    index + 1,
                    route,
                    section_group,
                ) {
                    Ok(nested) => nested,
                    Err(outcome) => return outcome,
                };
                index = nested.next_index;
                let group = group_from_first_level(
                    authored.body,
                    authored.body_start,
                    authored.body_start..line.raw.end,
                    line.line_number,
                    nested.notes,
                    true,
                );
                match group {
                    PlannedGroup::PendingSection { span } => {
                        if pending.is_none() {
                            pending = Some(pending_section(span));
                        }
                    }
                    PlannedGroup::PendingNotes => {
                        if pending.is_none() {
                            pending = Some(pending_section_notes());
                        }
                    }
                    PlannedGroup::Failed(outcome) => return outcome,
                    other => groups.push(other),
                }
            }
            super::draft::AuthoredLineClass::Item(_) => {
                index += 1;
            }
        }
    }
    if let Some(NotebookBodyOutcome::Pending {
        needs,
        placeholder,
        diagnostics,
        ..
    }) = pending
    {
        return match finish_create_groups(groups, route) {
            NotebookBodyOutcome::Ready(plan)
            | NotebookBodyOutcome::Pending { plan, .. }
            | NotebookBodyOutcome::Failed { plan, .. } => {
                NotebookBodyOutcome::Pending {
                    plan,
                    needs,
                    placeholder,
                    diagnostics,
                }
            }
        };
    }
    finish_create_groups(groups, route)
}

struct NestedCollect {
    notes: Vec<NotebookPlannedNote>,
    next_index: usize,
}

fn collect_nested_lines(
    item: &CaptureItem<'_>,
    start: usize,
    route: &str,
    section_group: bool,
) -> Result<NestedCollect, NotebookBodyOutcome> {
    let mut notes = Vec::new();
    let mut index = start;
    while index < item.lines.len() {
        let line = item.lines[index];
        match classify_authored_line(line.raw) {
            super::draft::AuthoredLineClass::EmptyOrPlaceholder => {
                index += 1;
            }
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == AuthoredDepth::Nested =>
            {
                notes.push(note_from_body(
                    authored.body,
                    line.raw.start..line.raw.end,
                    line.line_number,
                    false,
                ));
                index += 1;
            }
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == AuthoredDepth::First =>
            {
                break;
            }
            super::draft::AuthoredLineClass::Invalid => {
                if section_group && looks_like_deeper_bullet(line.raw.text) {
                    return Err(NotebookBodyOutcome::failed(error_at(
                        "section_bullet_depth",
                        section_bullet_depth_error(route, ""),
                        Some((line.raw.start, line.raw.end)),
                    )));
                }
                return Err(NotebookBodyOutcome::failed(error_at(
                    "invalid_child_line",
                    super::line::invalid_child_line_error(line.line_number),
                    Some((line.raw.start, line.raw.end)),
                )));
            }
            super::draft::AuthoredLineClass::Item(_) => {
                index += 1;
            }
        }
    }
    Ok(NestedCollect {
        notes,
        next_index: index,
    })
}

enum PlannedGroup {
    Top(NotebookPlannedNote),
    Section(NotebookPlannedSection),
    PendingSection { span: Range<usize> },
    PendingNotes,
    Failed(NotebookBodyOutcome),
}

fn group_from_first_level(
    body: &str,
    body_start: usize,
    source_range: Range<usize>,
    line_number: usize,
    notes: Vec<NotebookPlannedNote>,
    all_caps_as_section: bool,
) -> PlannedGroup {
    match scan_leading(body, body_start) {
        Some(SectionBulletHit::Incomplete { span }) => {
            PlannedGroup::PendingSection { span }
        }
        Some(SectionBulletHit::Complete { span, leftover, .. })
            if !leftover.is_empty() =>
        {
            PlannedGroup::Failed(NotebookBodyOutcome::failed(error_at(
                "section_bullet_text",
                section_bullet_text_error(),
                Some((span.start, span.end)),
            )))
        }
        Some(SectionBulletHit::Complete {
            span,
            selector,
            exact,
            authored,
            leftover: _,
        }) => {
            if notes.is_empty() {
                return PlannedGroup::PendingNotes;
            }
            PlannedGroup::Section(NotebookPlannedSection {
                selector,
                exact,
                authored_marker: authored,
                selector_span: span,
                line_number,
                notes,
                all_caps: false,
            })
        }
        Some(SectionBulletHit::Escaped { .. }) => {
            let text = unescape_escaped_body(body);
            let mut note =
                note_from_body(&text, source_range, line_number, true);
            note.detail = notes;
            PlannedGroup::Top(note)
        }
        None if all_caps_as_section
            && !notes.is_empty()
            && is_project_section_title(body) =>
        {
            let title = format_project_section_title(body);
            PlannedGroup::Section(NotebookPlannedSection {
                selector: title.clone(),
                exact: true,
                authored_marker: body.trim().to_string(),
                selector_span: source_range,
                line_number,
                notes,
                all_caps: true,
            })
        }
        None => {
            let mut note =
                note_from_body(body, source_range, line_number, false);
            note.detail = notes;
            PlannedGroup::Top(note)
        }
    }
}

fn finish_create_groups(
    groups: Vec<PlannedGroup>,
    route: &str,
) -> NotebookBodyOutcome {
    let mut ordered = Vec::new();
    for group in groups {
        match group {
            PlannedGroup::PendingSection { span } => {
                return pending_section(span);
            }
            PlannedGroup::PendingNotes => return pending_section_notes(),
            PlannedGroup::Failed(outcome) => return outcome,
            other => ordered.push(other),
        }
    }
    match merge_create_groups(ordered, route) {
        Ok(plan) => NotebookBodyOutcome::Ready(plan),
        Err(outcome) => outcome,
    }
}

fn merge_groups(
    groups: Vec<PlannedGroup>,
    existing: &[NotebookSection],
) -> NotebookBodyPlan {
    merge_create_groups(groups, "").unwrap_or_else(|_| NotebookBodyPlan {
        top_notes: Vec::new(),
        sections: existing
            .iter()
            .map(|heading| NotebookPlannedSection {
                selector: heading.title.clone(),
                exact: true,
                authored_marker: heading.title.clone(),
                selector_span: 0..0,
                line_number: heading.line_index,
                notes: Vec::new(),
                all_caps: false,
            })
            .collect(),
    })
}

fn merge_create_groups(
    groups: Vec<PlannedGroup>,
    route: &str,
) -> Result<NotebookBodyPlan, NotebookBodyOutcome> {
    let mut top_notes: Vec<NotebookPlannedNote> = Vec::new();
    let mut sections: Vec<NotebookPlannedSection> = Vec::new();
    let mut headings: Vec<NotebookSection> = Vec::new();
    for group in groups {
        match group {
            PlannedGroup::Top(note) => top_notes.push(note),
            PlannedGroup::Section(mut section) => {
                let exact = section.exact || section.all_caps;
                match resolve_notebook_section(
                    &section.selector,
                    exact,
                    &headings,
                ) {
                    NotebookSectionResolution::Existing { title, .. } => {
                        if let Some(found) = sections.iter_mut().find(|entry| {
                            normalize_task_text(&entry.selector)
                                .eq_ignore_ascii_case(&normalize_task_text(
                                    &title,
                                ))
                        }) {
                            found.notes.append(&mut section.notes);
                            continue;
                        }
                        section.selector = title;
                        headings.push(NotebookSection {
                            title: section.selector.clone(),
                            line_index: usize::MAX - headings.len(),
                        });
                        sections.push(section);
                    }
                    NotebookSectionResolution::Create { title } => {
                        section.selector = title.clone();
                        headings.push(NotebookSection {
                            title,
                            line_index: usize::MAX - headings.len(),
                        });
                        sections.push(section);
                    }
                    NotebookSectionResolution::NearMiss {
                        slug,
                        quoted_title,
                    } => {
                        return Err(NotebookBodyOutcome::failed(error_at(
                            "notebook_section_near_miss",
                            super::notebook::notebook_section_near_miss_message(
                                route,
                                &section.selector,
                                &slug,
                                &quoted_title,
                            ),
                            Some((
                                section.selector_span.start,
                                section.selector_span.end,
                            )),
                        )));
                    }
                    NotebookSectionResolution::Ambiguous { matches } => {
                        return Err(NotebookBodyOutcome::failed(error_at(
                            "notebook_section_ambiguous",
                            super::notebook::notebook_section_ambiguous_message(
                                route, &matches,
                            ),
                            Some((
                                section.selector_span.start,
                                section.selector_span.end,
                            )),
                        )));
                    }
                }
            }
            PlannedGroup::PendingSection { .. }
            | PlannedGroup::PendingNotes
            | PlannedGroup::Failed(_) => {}
        }
    }
    Ok(NotebookBodyPlan {
        top_notes,
        sections,
    })
}

fn note_from_body(
    body: &str,
    source_range: Range<usize>,
    line_number: usize,
    escaped: bool,
) -> NotebookPlannedNote {
    let (text, block_id) = strip_trailing_caret_id(body.trim());
    NotebookPlannedNote {
        text,
        block_id,
        detail: Vec::new(),
        source_range,
        line_number,
        escaped,
    }
}

fn error_at(
    code: &'static str,
    message: String,
    range: Option<(usize, usize)>,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code,
        message,
        range,
        rewrite: None,
    }
}

fn parse_hit(
    line: &str,
    start: usize,
    sigil: usize,
    escaped: bool,
) -> SectionBulletHit {
    let content_start = sigil + 1;
    let rest = &line[content_start..];
    if rest.starts_with('"') || rest.starts_with('“') {
        let lex = lex_exact_name_after_hash(rest, rest.len())
            .expect("quote opener yields a literal");
        let marker_end = content_start + lex.literal_len;
        let raw_end = if lex.complete { marker_end } else { line.len() };
        if escaped {
            return SectionBulletHit::Escaped {
                span: start..raw_end,
            };
        }
        if !lex.complete || lex.decoded.trim().is_empty() {
            return SectionBulletHit::Incomplete {
                span: start..raw_end,
            };
        }
        let leftover = leftover_after_selector(line, marker_end);
        let authored = line[start..marker_end].trim_start().to_string();
        return SectionBulletHit::Complete {
            span: start..marker_end,
            selector: lex.decoded,
            exact: true,
            leftover,
            authored,
        };
    }
    let candidate_end = token_end(line, content_start);
    if escaped {
        return SectionBulletHit::Escaped {
            span: start..candidate_end.max(content_start),
        };
    }
    let suffix = &line[content_start..candidate_end];
    if suffix.is_empty() {
        return SectionBulletHit::Incomplete {
            span: start..content_start,
        };
    }
    let parsed = super::tokens::parse_hash_section_and_id(suffix);
    let selector_end = content_start + parsed.section_len;
    let leftover = if parsed.has_caret {
        leftover_after_selector(line, selector_end)
    } else {
        leftover_after_selector(line, candidate_end)
    };
    if parsed.incomplete_section
        || parsed.section.as_ref().is_none_or(|s| s.is_empty())
    {
        return SectionBulletHit::Incomplete {
            span: start..selector_end.max(content_start),
        };
    }
    let selector = parsed.section.unwrap_or_default();
    let authored = line[start..selector_end].trim_start().to_string();
    SectionBulletHit::Complete {
        span: start..selector_end,
        selector,
        exact: parsed.exact,
        leftover,
        authored,
    }
}

fn leftover_after_selector(line: &str, end: usize) -> String {
    line.get(end..).unwrap_or("").trim().to_string()
}

fn shift_hit(hit: SectionBulletHit, base: usize) -> SectionBulletHit {
    let shift = |span: Range<usize>| (span.start + base)..(span.end + base);
    match hit {
        SectionBulletHit::Complete {
            span,
            selector,
            exact,
            leftover,
            authored,
        } => SectionBulletHit::Complete {
            span: shift(span),
            selector,
            exact,
            leftover,
            authored,
        },
        SectionBulletHit::Incomplete { span } => {
            SectionBulletHit::Incomplete { span: shift(span) }
        }
        SectionBulletHit::Escaped { span } => {
            SectionBulletHit::Escaped { span: shift(span) }
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

/// Extract accepted section operations from a capture item for the staged
/// catalog. Offsets are the selector span starts.
pub(crate) fn section_operations_from_item(
    item: &CaptureItem<'_>,
    route: &str,
    create_route: Option<&str>,
) -> Vec<NotebookSectionOperation> {
    let mut operations = Vec::new();
    if let Some(created) = create_route {
        match plan_create_item(item, created) {
            NotebookBodyOutcome::Ready(plan)
            | NotebookBodyOutcome::Pending { plan, .. } => {
                for section in plan.sections {
                    operations.push(NotebookSectionOperation {
                        offset: section.selector_span.start,
                        selector: section.selector,
                        exact: section.exact || section.all_caps,
                        route: created.to_string(),
                    });
                }
            }
            NotebookBodyOutcome::Failed { plan, .. } => {
                for section in plan.sections {
                    operations.push(NotebookSectionOperation {
                        offset: section.selector_span.start,
                        selector: section.selector,
                        exact: section.exact || section.all_caps,
                        route: created.to_string(),
                    });
                }
            }
        }
        return operations;
    }
    if item.list.is_some()
        && let NotebookBodyOutcome::Ready(plan) = plan_list_item(item, route)
    {
        for section in plan.sections {
            operations.push(NotebookSectionOperation {
                offset: section.selector_span.start,
                selector: section.selector,
                exact: section.exact,
                route: route.to_string(),
            });
        }
    }
    operations
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookSectionOperation {
    pub(crate) offset: usize,
    pub(crate) selector: String,
    pub(crate) exact: bool,
    pub(crate) route: String,
}
