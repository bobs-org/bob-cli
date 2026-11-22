//! Requirement positions for authored capture bullets.
//!
//! One pure resolver answers every requirement question for both capture
//! parsers: `bob capture` item parsing and the `capture-parse` editor
//! parser. Each parser builds its authored bullet list, derives a
//! [`ReqHost`] from the item's resolved shape (never from the vault), and
//! calls [`resolve`]. Neither parser re-implements the rules.
//!
//! A checkbox bullet (`[c]` followed by whitespace or end of line) is a
//! requirement where it lands directly under a task or another
//! requirement. Levels 1 and 2 are admitted exactly as before; a deeper
//! level is admitted only beneath a requirement chain: its parent is a
//! requirement, or a note whose own parent is a requirement.
//!
//! Before an item's host is known, the parser loops use [`LevelGate`], a
//! lexical pre-check that only lets a deeper line through when the
//! checkbox shape of its ancestors could make it a requirement chain.

use super::draft::{
    classify_authored_line, marker_only_placeholder_after_prefix, AuthoredLine,
    AuthoredLineClass, TWO_LEVEL_CAP,
};
use super::editor_model::{
    Diagnostic, EditorMode, ReqMarker, Severity, Span, SpanKind,
};
use super::line::invalid_child_line_error;
use super::model::{
    AuthoredDepth, AuthoredSubBullet, CaptureItem, CaptureKind, ReqBulletInfo,
};
use super::project_tasks::split_leading_checkbox;
use crate::native::task_reqs::{copy, MAX_REQ_DEPTH};

/// Authored levels admitted without a requirement chain on ordinary items.
pub(crate) const FREE_LEVELS: u8 = 2;
/// Authored levels admitted without a requirement chain on a `@route`
/// task-list bullet, counted after the list shift: the bullet is the item
/// line, so its notes gain no extra nesting.
pub(crate) const LIST_FREE_LEVELS: u8 = 1;

/// Where an item's authored bullets land, derived from its resolved shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReqHost {
    /// The item creates a task: plain text, `@route`, `@route^id`, a
    /// task-creating `@route:id`, or a task-list bullet.
    NewTask,
    /// A note-less `@route+id` whose first-level bullets are its notes.
    ExistingTask,
    /// `@route+id` whose item text starts with a checkbox: the item line
    /// is itself a requirement of the existing task.
    SameLineReq,
    /// `@route+id <note text>`: bullets nest under that note.
    SingleNote,
    /// `@route+id#drawer` or a forced `--drawer`: every bullet is inside a
    /// drawer.
    Drawer,
    /// A sealed project note: first-level bullets are `## Tasks` rows,
    /// sections, or moves.
    ProjectNote,
    /// Every other item: requirements never apply.
    Ineligible,
}

impl ReqHost {
    /// The host of a `bob capture` item from its resolved kind and item
    /// text.
    pub(crate) fn from_kind(kind: &CaptureKind, body: &str) -> Self {
        match kind {
            CaptureKind::Task
            | CaptureKind::TaskWithBlockId { .. }
            | CaptureKind::Pomodoro { close: None, .. } => Self::NewTask,
            CaptureKind::ProjectNote { .. } => Self::ProjectNote,
            CaptureKind::SubBullet { section, .. } => {
                Self::sub_bullet(section.is_some(), body)
            }
            _ => Self::Ineligible,
        }
    }

    /// The host of a `capture-parse` item from its editor mode, item text,
    /// and glued drawer section. Reviewed together with
    /// [`ReqHost::from_kind`]: the two mappings must agree.
    pub(crate) fn from_mode(
        mode: EditorMode,
        body: &str,
        glued_section: Option<&str>,
    ) -> Self {
        match mode {
            EditorMode::Task | EditorMode::PomodoroTask => Self::NewTask,
            EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => {
                Self::ProjectNote
            }
            EditorMode::SubBullet => Self::sub_bullet(
                glued_section.is_some_and(|section| !section.is_empty()),
                body,
            ),
            _ => Self::Ineligible,
        }
    }

    fn sub_bullet(drawer: bool, body: &str) -> Self {
        if drawer {
            Self::Drawer
        } else if body.trim().is_empty() {
            Self::ExistingTask
        } else if leading_checkbox(body).is_some() {
            Self::SameLineReq
        } else {
            Self::SingleNote
        }
    }

    /// Hosts where requirements can nest, so a refused deeper line's copy
    /// teaches the requirement chain.
    pub(crate) fn nests_reqs(self) -> bool {
        matches!(
            self,
            Self::NewTask
                | Self::ExistingTask
                | Self::SameLineReq
                | Self::ProjectNote
        )
    }
}

/// The checkbox mark that starts `text`, when it starts with `[c]`
/// followed by whitespace or end of line.
pub(crate) fn leading_checkbox(text: &str) -> Option<char> {
    split_leading_checkbox(text.trim_start()).0
}

/// Whether `text` is a checkbox with nothing after it.
pub(crate) fn is_empty_checkbox(text: &str) -> bool {
    let (mark, rest) = split_leading_checkbox(text.trim_start());
    mark.is_some() && rest.trim().is_empty()
}

/// The byte range a `req_marker` span covers inside `text` (which starts
/// at absolute offset `base` and may begin with whitespace): the `[`, the
/// mark, the `]`, and one following space or tab when present.
pub(crate) fn checkbox_range(
    text: &str,
    base: usize,
) -> Option<(usize, usize)> {
    leading_checkbox(text)?;
    let start = text.len() - text.trim_start().len();
    let rest = &text[start..];
    let mut chars = rest.char_indices();
    chars.next()?;
    chars.next()?;
    let (close, _) = chars.next()?;
    let mut end = start + close + 1;
    if matches!(text.as_bytes().get(end), Some(b' ' | b'\t')) {
        end += 1;
    }
    Some((base + start, base + end))
}

/// A deeper marker-only row (`    - `), which deeper typing produces. In
/// task hosts it is a harmless placeholder like a `- ` row.
pub(crate) fn is_deep_placeholder_row(text: &str) -> bool {
    let spaces = text.bytes().take_while(|byte| *byte == b' ').count();
    spaces >= 4
        && spaces % 2 == 0
        && u8::try_from(spaces / 2 + 1)
            .is_ok_and(|level| level <= AuthoredDepth::MAX_LEVEL)
        && marker_only_placeholder_after_prefix(text, spaces)
}

/// `invalid_child_line` copy. Hosts where requirements nest add the hint
/// that deeper bullets nest only beneath a requirement; every other host
/// keeps today's exact sentence.
pub(crate) fn invalid_child_line_message(
    line_number: usize,
    nests_reqs: bool,
) -> String {
    let base = invalid_child_line_error(line_number);
    if nests_reqs {
        format!(
            "{base}; deeper bullets nest only beneath a requirement (`- [ ] …`)"
        )
    } else {
        base
    }
}

/// The `bob capture` usage code for a requirement-position message.
pub(crate) fn code_for_message(message: &str) -> Option<&'static str> {
    match message {
        copy::REQ_IN_DRAWER => Some("req_in_drawer"),
        copy::REQ_PARENT => Some("req_parent"),
        copy::REQ_STATE => Some("req_state"),
        copy::REQ_DEPTH => Some("req_depth"),
        _ if message.ends_with(
            "; deeper bullets nest only beneath a requirement (`- [ ] …`)",
        ) =>
        {
            Some("invalid_child_line")
        }
        _ => None,
    }
}

/// Lexical admission of deeper authored levels, before the item's host is
/// known. Levels up to `free_levels` always pass (the caller keeps its
/// orphan check); a deeper line passes only directly beneath a checkbox
/// bullet, or beneath a plain bullet whose parent is a checkbox bullet,
/// and never as a jump of more than one level. [`resolve`] re-checks every
/// admitted line once the host is known.
#[derive(Debug, Clone)]
pub(crate) struct LevelGate {
    free_levels: u8,
    /// The ancestor chain of the next line: strictly increasing levels,
    /// each with whether that bullet starts with a checkbox.
    stack: Vec<(u8, bool)>,
}

impl LevelGate {
    pub(crate) fn new(free_levels: u8) -> Self {
        Self {
            free_levels,
            stack: Vec::new(),
        }
    }

    pub(crate) fn admits(&self, level: u8) -> bool {
        if level <= self.free_levels {
            return true;
        }
        let ancestors: Vec<(u8, bool)> = self
            .stack
            .iter()
            .copied()
            .filter(|(ancestor, _)| *ancestor < level)
            .collect();
        let Some((parent_level, parent_checkbox)) = ancestors.last().copied()
        else {
            return false;
        };
        if parent_level + 1 != level {
            return false;
        }
        if parent_checkbox {
            return true;
        }
        ancestors.len() >= 2 && {
            let (grand_level, grand_checkbox) = ancestors[ancestors.len() - 2];
            grand_level + 2 == level && grand_checkbox
        }
    }

    pub(crate) fn push(&mut self, level: u8, checkbox: bool) {
        while self
            .stack
            .last()
            .is_some_and(|(ancestor, _)| *ancestor >= level)
        {
            self.stack.pop();
        }
        self.stack.push((level, checkbox));
    }
}

/// One authored continuation line the item's parsers admit, with its
/// effective level (after any list shift).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdmittedLine<'a> {
    pub(crate) authored: AuthoredLine<'a>,
    pub(crate) level: u8,
}

/// The lexical admission every task-host helper shares with the parser
/// loops: for each line of `item` (index 0 is the item line, always
/// `None`), the authored line and its effective level when the parsers
/// admit it. Levels within the free levels always pass; deeper ones only
/// beneath a requirement chain ([`LevelGate`]). List items count levels
/// after the shift. Empty, placeholder, invalid, and refused lines map to
/// `None`, so on lines no deeper than two this matches the two-level
/// classifier exactly.
pub(crate) fn admitted_authored_lines<'a>(
    item: &CaptureItem<'a>,
) -> Vec<Option<AdmittedLine<'a>>> {
    let list = item.list.is_some();
    let mut gate =
        LevelGate::new(if list { LIST_FREE_LEVELS } else { FREE_LEVELS });
    let mut admitted = Vec::with_capacity(item.lines.len());
    for (index, line) in item.lines.iter().enumerate() {
        if index == 0 {
            admitted.push(None);
            continue;
        }
        let AuthoredLineClass::Item(authored) =
            classify_authored_line(line.raw)
        else {
            admitted.push(None);
            continue;
        };
        let level = if list {
            authored.depth.shallower().level()
        } else {
            authored.depth.level()
        };
        if !gate.admits(level) {
            admitted.push(None);
            continue;
        }
        gate.push(level, leading_checkbox(authored.body).is_some());
        admitted.push(Some(AdmittedLine { authored, level }));
    }
    admitted
}

/// The bullet-prefix cap for one authored child line's edge scans (type
/// markers, directives): a line the parsers admit loses its prefix at any
/// level, and every other line keeps the two-level reading, so a refused
/// deeper line scans exactly as before.
pub(crate) fn child_bullet_cap(admitted: bool) -> u8 {
    if admitted {
        AuthoredDepth::MAX_LEVEL
    } else {
        TWO_LEVEL_CAP
    }
}

/// One authored bullet as the resolver sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReqBullet {
    /// Authored level after any list shift.
    pub(crate) level: u8,
    /// The leading checkbox mark, when the bullet starts with one.
    pub(crate) mark: Option<char>,
    /// A first-level drawer header (`- #slug`, or ALL-CAPS with nested
    /// bullets) or, in a project note, a section.
    pub(crate) header: bool,
    /// A checkbox with no text after it.
    pub(crate) empty: bool,
    /// 1-based draft line, for copy that names the line.
    pub(crate) line_number: usize,
}

/// What an authored bullet is, once the host is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReqRole {
    Note,
    /// A requirement; depth 1 is a direct child of its owning task.
    Req {
        depth: u8,
    },
    DrawerHeader,
    DrawerEntry,
    ProjectRow,
    /// Refused here, or inside a refused bullet's subtree.
    Refused,
}

/// One refused position. `bullet` is `None` for the item line itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqRefusal {
    pub(crate) bullet: Option<usize>,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqResolution {
    /// The item line's requirement depth on an admitted same-line form.
    pub(crate) item_line: Option<u8>,
    /// One role per input bullet.
    pub(crate) roles: Vec<ReqRole>,
    /// Every refusal in draft order.
    pub(crate) refusals: Vec<ReqRefusal>,
}

impl ReqResolution {
    /// Whether bullet `index` is an admitted placeholder: an empty
    /// requirement with no nested bullets, which writes nothing.
    pub(crate) fn is_placeholder(
        &self,
        bullets: &[ReqBullet],
        index: usize,
    ) -> bool {
        matches!(self.roles.get(index), Some(ReqRole::Req { .. }))
            && bullets[index].empty
            && !has_children(bullets, index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    Owner,
    ProjectRoot,
    Role(ReqRole),
}

fn has_children(bullets: &[ReqBullet], index: usize) -> bool {
    bullets
        .get(index + 1)
        .is_some_and(|next| next.level > bullets[index].level)
}

/// Resolve every bullet's role and refusal. `item_mark` is the item
/// line's leading checkbox (same-line form and drawer items);
/// `free_levels` is [`FREE_LEVELS`] or, after a list shift,
/// [`LIST_FREE_LEVELS`].
pub(crate) fn resolve(
    host: ReqHost,
    item_mark: Option<char>,
    free_levels: u8,
    bullets: &[ReqBullet],
) -> ReqResolution {
    let mut refusals = Vec::new();
    let mut roles = Vec::with_capacity(bullets.len());
    let mut item_line = None;
    let root = match host {
        ReqHost::Ineligible => {
            for (index, bullet) in bullets.iter().enumerate() {
                if bullet.level > free_levels {
                    refusals.push(ReqRefusal {
                        bullet: Some(index),
                        code: "invalid_child_line",
                        message: invalid_child_line_message(
                            bullet.line_number,
                            false,
                        ),
                    });
                    roles.push(ReqRole::Refused);
                } else {
                    roles.push(ReqRole::Note);
                }
            }
            return ReqResolution {
                item_line,
                roles,
                refusals,
            };
        }
        ReqHost::NewTask | ReqHost::ExistingTask => Node::Owner,
        ReqHost::ProjectNote => Node::ProjectRoot,
        ReqHost::SingleNote => Node::Role(ReqRole::Note),
        ReqHost::Drawer => {
            if item_mark.is_some() {
                refusals.push(refusal(None, "req_in_drawer"));
            }
            Node::Role(ReqRole::DrawerHeader)
        }
        ReqHost::SameLineReq => {
            if item_mark.is_some_and(|mark| mark != ' ') {
                refusals.push(refusal(None, "req_state"));
                Node::Role(ReqRole::Refused)
            } else {
                item_line = Some(1);
                Node::Role(ReqRole::Req { depth: 1 })
            }
        }
    };
    let nests = host.nests_reqs();
    let mut stack: Vec<(u8, Node)> = vec![(0, root)];
    for (index, bullet) in bullets.iter().enumerate() {
        while stack.len() > 1
            && stack
                .last()
                .is_some_and(|(level, _)| *level >= bullet.level)
        {
            stack.pop();
        }
        let (parent_level, parent) = *stack.last().expect("root stays");
        let grand = stack
            .len()
            .checked_sub(2)
            .and_then(|position| stack.get(position))
            .map(|(_, node)| *node);
        let role = role_for(
            index,
            bullet,
            parent_level,
            parent,
            grand,
            free_levels,
            nests,
            bullets,
            &mut refusals,
        );
        roles.push(role);
        stack.push((bullet.level, Node::Role(role)));
    }
    ReqResolution {
        item_line,
        roles,
        refusals,
    }
}

#[allow(clippy::too_many_arguments)]
fn role_for(
    index: usize,
    bullet: &ReqBullet,
    parent_level: u8,
    parent: Node,
    grand: Option<Node>,
    free_levels: u8,
    nests: bool,
    bullets: &[ReqBullet],
    refusals: &mut Vec<ReqRefusal>,
) -> ReqRole {
    if parent == Node::Role(ReqRole::Refused) {
        return ReqRole::Refused;
    }
    let deep = bullet.level > free_levels;
    if bullet.level > parent_level + 1 {
        // A jump. At admitted levels the parser already reported the
        // orphaned bullet; deeper jumps stay `invalid_child_line`.
        if deep {
            refusals.push(ReqRefusal {
                bullet: Some(index),
                code: "invalid_child_line",
                message: invalid_child_line_message(bullet.line_number, nests),
            });
        }
        return ReqRole::Refused;
    }
    let is_req = |node: Option<Node>| {
        matches!(node, Some(Node::Role(ReqRole::Req { .. })))
    };
    let is_note = |node: Node| node == Node::Role(ReqRole::Note);
    if deep && !(is_req(Some(parent)) || (is_note(parent) && is_req(grand))) {
        refusals.push(ReqRefusal {
            bullet: Some(index),
            code: "invalid_child_line",
            message: invalid_child_line_message(bullet.line_number, nests),
        });
        return ReqRole::Refused;
    }
    let role = match (bullet.mark, parent) {
        (Some(_), Node::Owner | Node::Role(ReqRole::ProjectRow)) => {
            ReqRole::Req { depth: 1 }
        }
        (Some(_), Node::Role(ReqRole::Req { depth })) => ReqRole::Req {
            depth: depth.saturating_add(1),
        },
        (Some(_), Node::ProjectRoot) => ReqRole::ProjectRow,
        (Some(_), Node::Role(ReqRole::DrawerHeader | ReqRole::DrawerEntry)) => {
            refusals.push(refusal(Some(index), "req_in_drawer"));
            return ReqRole::Refused;
        }
        (Some(_), Node::Role(ReqRole::Note | ReqRole::Refused)) => {
            refusals.push(refusal(Some(index), "req_parent"));
            return ReqRole::Refused;
        }
        (None, Node::Owner) if bullet.header => ReqRole::DrawerHeader,
        (None, Node::ProjectRoot) if bullet.header => ReqRole::DrawerHeader,
        (None, Node::ProjectRoot) => ReqRole::ProjectRow,
        (None, Node::Role(ReqRole::DrawerHeader | ReqRole::DrawerEntry)) => {
            ReqRole::DrawerEntry
        }
        (None, _) => ReqRole::Note,
    };
    if let ReqRole::Req { depth } = role {
        if bullet.mark.is_some_and(|mark| mark != ' ') {
            refusals.push(refusal(Some(index), "req_state"));
            return ReqRole::Refused;
        }
        if usize::from(depth) > MAX_REQ_DEPTH {
            refusals.push(refusal(Some(index), "req_depth"));
            return ReqRole::Refused;
        }
        if bullet.empty && has_children(bullets, index) {
            refusals.push(ReqRefusal {
                bullet: Some(index),
                code: "empty_child_after_markers",
                message: super::line::empty_child_after_markers_error(
                    bullet.line_number,
                ),
            });
        }
    }
    role
}

fn refusal(bullet: Option<usize>, code: &'static str) -> ReqRefusal {
    let message = match code {
        "req_in_drawer" => copy::REQ_IN_DRAWER,
        "req_parent" => copy::REQ_PARENT,
        "req_state" => copy::REQ_STATE,
        "req_depth" => copy::REQ_DEPTH,
        _ => unreachable!("req refusal codes only"),
    };
    ReqRefusal {
        bullet,
        code,
        message: message.to_string(),
    }
}

/// Resolve an execution item's requirement positions in place: refuse the
/// first problem in draft order, record each admitted requirement's depth,
/// and drop admitted placeholders (an empty `- [ ] ` with nothing nested),
/// which write nothing. `marks` holds each bullet's authored leading
/// checkbox, aligned with `sub_bullets` and `line_numbers`.
pub(crate) fn apply_to_execution(
    host: ReqHost,
    item_body: &str,
    free_levels: u8,
    sub_bullets: &mut Vec<AuthoredSubBullet>,
    line_numbers: &[usize],
    marks: &[Option<char>],
) -> Result<(), ReqRefusal> {
    let bullets: Vec<ReqBullet> = sub_bullets
        .iter()
        .enumerate()
        .map(|(index, bullet)| {
            let checkbox = bullet.drawer.is_none()
                && bullet.move_task.is_none()
                && bullet.move_choice.is_none();
            let mark = checkbox
                .then(|| marks.get(index).copied().flatten())
                .flatten();
            ReqBullet {
                level: bullet.depth.level(),
                mark,
                header: bullet.depth.is_first() && bullet.drawer.is_some(),
                empty: mark.is_some() && is_empty_checkbox(&bullet.body),
                line_number: line_numbers.get(index).copied().unwrap_or(0),
            }
        })
        .collect();
    let item_mark = match host {
        ReqHost::SameLineReq | ReqHost::Drawer => leading_checkbox(item_body),
        _ => None,
    };
    let resolution = resolve(host, item_mark, free_levels, &bullets);
    if let Some(first) = resolution.refusals.first() {
        return Err(first.clone());
    }
    for (bullet, role) in sub_bullets.iter_mut().zip(&resolution.roles) {
        bullet.req = match role {
            ReqRole::Req { depth } => Some(ReqBulletInfo { depth: *depth }),
            _ => None,
        };
    }
    for index in (0..bullets.len()).rev() {
        if resolution.is_placeholder(&bullets, index) {
            sub_bullets.remove(index);
        }
    }
    Ok(())
}

/// Re-resolve a parsed item after a forced `--task` / `--task-ref` /
/// `--drawer` target rewrote its kind: the parse resolved its bullets as a
/// new task's, where every surviving checkbox bullet is a requirement.
pub(crate) fn reresolve_forced_target(
    kind: &CaptureKind,
    item_body: &str,
    sub_bullets: &mut Vec<AuthoredSubBullet>,
) -> Result<(), ReqRefusal> {
    let marks: Vec<Option<char>> = sub_bullets
        .iter()
        .map(|bullet| bullet.req.map(|_| ' '))
        .collect();
    let host = ReqHost::from_kind(kind, item_body);
    apply_to_execution(host, item_body, FREE_LEVELS, sub_bullets, &[], &marks)
}

/// One editor bullet's line facts, aligned with the item's sub-bullets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReqLine {
    pub(crate) line_number: usize,
    /// The bullet's full physical line range.
    pub(crate) range: (usize, usize),
    /// The authored leading checkbox mark.
    pub(crate) mark: Option<char>,
    /// The authored checkbox's `req_marker` range.
    pub(crate) mark_range: Option<(usize, usize)>,
}

impl ReqLine {
    /// Line facts for an authored bullet body starting at `body_start`.
    pub(crate) fn new(
        line_number: usize,
        range: (usize, usize),
        body: &str,
        body_start: usize,
    ) -> Self {
        Self {
            line_number,
            range,
            mark: leading_checkbox(body),
            mark_range: checkbox_range(body, body_start),
        }
    }
}

/// Resolve an editor item's requirement positions: push every refusal as
/// an error diagnostic on its bullet's full line (the item line for a
/// same-line refusal), record each admitted requirement's depth and
/// `req_marker` span, and drop admitted placeholders from the
/// sub-bullets (their span stays). Returns the item's requirement markers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_to_editor(
    host: ReqHost,
    item_body: &str,
    item_mark_range: Option<(usize, usize)>,
    item_line_range: (usize, usize),
    free_levels: u8,
    sub_bullets: &mut Vec<AuthoredSubBullet>,
    lines: &[ReqLine],
    spans: &mut Vec<Span>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ReqMarker> {
    let bullets: Vec<ReqBullet> = sub_bullets
        .iter()
        .zip(lines)
        .map(|(bullet, line)| {
            let checkbox = bullet.drawer.is_none()
                && bullet.move_task.is_none()
                && bullet.move_choice.is_none();
            let mark = if checkbox { line.mark } else { None };
            ReqBullet {
                level: bullet.depth.level(),
                mark,
                header: bullet.depth.is_first() && bullet.drawer.is_some(),
                empty: mark.is_some() && is_empty_checkbox(&bullet.body),
                line_number: line.line_number,
            }
        })
        .collect();
    let item_mark = match host {
        ReqHost::SameLineReq | ReqHost::Drawer => leading_checkbox(item_body),
        _ => None,
    };
    let resolution = resolve(host, item_mark, free_levels, &bullets);
    for refusal in &resolution.refusals {
        let range = match refusal.bullet {
            Some(index) => lines[index].range,
            None => item_line_range,
        };
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: refusal.code,
            message: refusal.message.clone(),
            range: Some(range),
            rewrite: None,
        });
    }
    let mut markers = Vec::new();
    if let (Some(depth), Some((start, end))) =
        (resolution.item_line, item_mark_range)
    {
        markers.push(ReqMarker { start, end, depth });
    }
    for (index, role) in resolution.roles.iter().enumerate() {
        let ReqRole::Req { depth } = *role else {
            continue;
        };
        sub_bullets[index].req = Some(ReqBulletInfo { depth });
        if let Some((start, end)) = lines[index].mark_range {
            markers.push(ReqMarker { start, end, depth });
        }
    }
    for marker in &markers {
        spans.push(Span {
            start: marker.start,
            end: marker.end,
            kind: SpanKind::ReqMarker,
            project_note: None,
            title: None,
            key: None,
            section_action: None,
        });
    }
    for index in (0..bullets.len()).rev() {
        if resolution.is_placeholder(&bullets, index) {
            sub_bullets.remove(index);
        }
    }
    markers
}
