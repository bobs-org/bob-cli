//! Sub-bullet/note placement and managed-log parsing.
use super::*;
use crate::native::task_reqs;

pub(super) fn plan_pomodoro_note_capture(
    planner: &mut CaptureBatchPlanner,
    day_file: &Path,
    capture_block: &str,
) -> Result<CaptureWritePlan, CaptureError> {
    if !day_file.is_file() {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }

    let contents = planner.read_existing(day_file)?;
    let (updated, placement, pomodoro_line, pomodoro_text) =
        insert_pomodoro_child_block(
            &contents,
            capture_block,
            PomodoroSelection::CurrentOrLastCompleted,
        )?;
    planner.stage(day_file, updated)?;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: None,
        pomodoro_note: Some(PomodoroNoteDetails {
            day_file: day_file.display().to_string(),
            pomodoro_line: pomodoro_line + 1,
            pomodoro_text,
        }),
        toggle: None,
        pomodoro_link: None,
        // Pomodoro notes rely on block auto-detection.
        pomodoro_refs: Vec::new(),
    })
}

pub(super) fn plan_sub_bullet_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    sub_bullet_target: &SubBulletTarget,
    section_selector: Option<&DrawerSelector>,
    capture_block: &str,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<CaptureWritePlan, CaptureError> {
    let contents = planner.read_existing(target)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = resolve_sub_bullet_parent(&scan, route, sub_bullet_target)?;

    let (contents, insertion_offset, indentation, drawer) =
        if let Some(selector) = section_selector {
            if capture_drawers::is_prj_parent(&contents, parent) {
                return plan_prj_section_capture(
                    planner,
                    bob_dir,
                    target,
                    route,
                    &contents,
                    parent,
                    selector,
                    capture_block,
                    warnings,
                );
            }
            let catalog = capture_drawers::load_drawer_catalog(bob_dir)
                .unwrap_or_default();
            let drawers = capture_drawers::drawers(&contents, parent);
            let resolved = capture_drawers::resolve_drawer_for_write(
                &drawers,
                &selector.text,
                selector.exact,
                &catalog,
                route,
                parent.block_id.as_deref(),
                capture_drawers::DrawerScope::Task,
            )
            .map_err(|error| {
                CaptureError::usage_code(error.message.clone(), error.code)
            })?;
            if let Some(message) = resolved.warning.clone() {
                warnings.push(CaptureWarning::Structured(
                    StructuredCaptureWarning {
                        code: "drawer_near_catalog",
                        label: None,
                        count: None,
                        message,
                    },
                ));
            }
            let (contents, drawer, created) =
                capture_drawers::ensure_drawer(&contents, parent, &resolved)
                    .map_err(CaptureError::io)?;
            let insertion =
                capture_drawers::drawer_insertion(&contents, &drawer);
            (
                contents,
                insertion.offset,
                insertion.indentation,
                Some(DrawerJson {
                    title: drawer.title,
                    key: drawer.key,
                    marker: drawer.marker,
                    created,
                }),
            )
        } else {
            // The block-layout module owns the slot: a loose note keeps the
            // tail slot before the managed logs, and a task that owns reqs
            // keeps its notes in `NOTES` (`docs/task-requirements.md` §3).
            match task_block_layout::note_home(&contents, parent, &settings) {
                task_block_layout::NoteHome::Loose(slot) => {
                    (contents, slot.offset, slot.indent, None)
                }
                task_block_layout::NoteHome::Notes(drawer) => {
                    if capture_drawers::is_prj_parent(&contents, parent) {
                        return prj_section_write_plan(
                            planner,
                            target,
                            &contents,
                            parent,
                            drawer,
                            false,
                            capture_block,
                        );
                    }
                    let insertion =
                        capture_drawers::drawer_insertion(&contents, &drawer);
                    (
                        contents,
                        insertion.offset,
                        insertion.indentation,
                        Some(DrawerJson {
                            title: drawer.title,
                            key: drawer.key,
                            marker: drawer.marker,
                            created: false,
                        }),
                    )
                }
                task_block_layout::NoteHome::NewNotes => {
                    let (contents, drawer, created) =
                        task_block_layout::ensure_notes_drawer(
                            &contents, parent,
                        )
                        .map_err(CaptureError::io)?;
                    if capture_drawers::is_prj_parent(&contents, parent) {
                        return prj_section_write_plan(
                            planner,
                            target,
                            &contents,
                            parent,
                            drawer,
                            created,
                            capture_block,
                        );
                    }
                    let insertion =
                        capture_drawers::drawer_insertion(&contents, &drawer);
                    (
                        contents,
                        insertion.offset,
                        insertion.indentation,
                        Some(DrawerJson {
                            title: drawer.title,
                            key: drawer.key,
                            marker: drawer.marker,
                            created,
                        }),
                    )
                }
            }
        };
    let indented_block = capture_block
        .split('\n')
        .map(|line| format!("{indentation}{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let addition = insertion_text_preserving_line_endings(
        &contents,
        insertion_offset,
        &indented_block,
    );
    let placement = if insertion_offset >= contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    let details = SubBulletCaptureDetails {
        block_id: parent.block_id.clone(),
        parent_line: parent.line_index + 1,
        parent_text: parent.description.clone(),
        drawer,
        parent_status_symbol: parent.status_symbol,
        parent_status_name: parent.status_name.clone(),
    };

    let updated_target = insert_at(&contents, insertion_offset, &addition);
    planner.stage(target, updated_target)?;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: Some(details),
        pomodoro_note: None,
        toggle: None,
        pomodoro_link: None,
        pomodoro_refs: Vec::new(),
    })
}

/// Insert requirement blocks under an existing task and report the
/// first-requirement scoop. Capture calls this before planning any notes in
/// the same draft, so only notes that existed before this item are scooped.
pub(super) fn plan_insert_reqs_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    sub_bullet_target: &SubBulletTarget,
    section_selector: Option<&DrawerSelector>,
    blocks: &[String],
    dry_run: bool,
) -> Result<(CaptureWritePlan, RequirementWriteJson), CaptureError> {
    if section_selector.is_some() {
        return Err(CaptureError::usage_code(
            task_reqs::copy::REQ_IN_DRAWER,
            "req_in_drawer",
        ));
    }

    let contents = planner.read_existing(target)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = resolve_sub_bullet_parent(&scan, route, sub_bullet_target)?;
    let parent_preimage = line_spans(&contents)
        .get(parent.line_index)
        .map(|line| line.text)
        .ok_or_else(|| {
            CaptureError::io(
                "sub-bullet capture invariant failed: parent line is missing",
            )
        })?;
    let write = task_block_layout::insert_reqs(
        &contents,
        task_block_layout::ReqTarget {
            line_index: parent.line_index,
            preimage: parent_preimage,
        },
        blocks,
        &settings,
    )
    .map_err(|refusal| {
        CaptureError::usage_code(refusal.message, refusal.code)
    })?;

    let placement = if write.first_line >= line_spans(&contents).len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    let details = SubBulletCaptureDetails {
        block_id: parent.block_id.clone(),
        parent_line: parent.line_index + 1,
        parent_text: parent.description.clone(),
        drawer: None,
        parent_status_symbol: parent.status_symbol,
        parent_status_name: parent.status_name.clone(),
    };
    let report = RequirementWriteJson::from_insert(
        parent.description.clone(),
        write.added,
        write.scooped,
        write.notes_created,
        dry_run,
    );
    planner.stage(target, write.updated)?;

    Ok((
        CaptureWritePlan {
            placement,
            pomodoro: None,
            sub_bullet: Some(details),
            pomodoro_note: None,
            toggle: None,
            pomodoro_link: None,
            pomodoro_refs: Vec::new(),
        },
        report,
    ))
}

fn resolve_sub_bullet_parent<'a>(
    scan: &'a note_tasks::NoteTaskScan,
    route: &str,
    sub_bullet_target: &SubBulletTarget,
) -> Result<&'a note_tasks::NoteTask, CaptureError> {
    match sub_bullet_target {
        SubBulletTarget::BlockId(block_id) => match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => Ok(task),
            BlockIdLookup::NotATask {
                line_index,
                excerpt,
            } => Err(CaptureError::io(format!(
                "^{block_id} in {route}.md is not a task (line {}: {excerpt})",
                line_index + 1
            ))),
            BlockIdLookup::Duplicate(count) => Err(CaptureError::io(
                format!(
                    "block ID ^{block_id} appears {count} times in {route}.md; make it unique before capturing"
                ),
            )),
            BlockIdLookup::Missing => {
                let choices = format!(
                    "run 'bob capture-tasks -r {route}' to list task block IDs"
                );
                let message = match scan.suggest_block_id(block_id) {
                    Some(suggestion) => format!(
                        "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices})"
                    ),
                    None => format!(
                        "no task with block ID ^{block_id} in {route}.md ({choices})"
                    ),
                };
                Err(CaptureError::io(message))
            }
        },
        SubBulletTarget::Ref { line, digest } => match scan.by_ref(*line, digest) {
            RefLookup::Found(task) => Ok(task),
            RefLookup::Stale => Err(CaptureError::io(format!(
                "the selected task is no longer in {route}.md; rerun the task picker"
            ))),
            RefLookup::Ambiguous => Err(CaptureError::io(format!(
                "the selected task matches more than one line in {route}.md; rerun the task picker"
            ))),
        },
    }
}

/// Insert a column-zero capture block into a project `##` section. Shared by
/// explicit `#drawer` routing and implicit NOTES filing on a `^prj` owner.
fn insert_into_prj_section(
    contents: &str,
    drawer: &capture_drawers::Drawer,
    capture_block: &str,
) -> (String, Placement) {
    let (offset, empty) = capture_drawers::h2_insert_position(contents, drawer);
    let addition = if empty {
        empty_section_insertion_text(contents, offset, capture_block)
    } else {
        insertion_text_preserving_line_endings(contents, offset, capture_block)
    };
    let placement = if offset >= contents.len() {
        Placement::Appended
    } else {
        Placement::Inserted
    };
    (insert_at(contents, offset, &addition), placement)
}

fn prj_section_write_plan(
    planner: &mut CaptureBatchPlanner,
    target: &Path,
    contents: &str,
    parent: &note_tasks::NoteTask,
    drawer: capture_drawers::Drawer,
    created: bool,
    capture_block: &str,
) -> Result<CaptureWritePlan, CaptureError> {
    let (updated_target, placement) =
        insert_into_prj_section(contents, &drawer, capture_block);
    let details = SubBulletCaptureDetails {
        block_id: parent.block_id.clone(),
        parent_line: parent.line_index + 1,
        parent_text: parent.description.clone(),
        drawer: Some(DrawerJson {
            title: drawer.title,
            key: drawer.key,
            marker: drawer.marker,
            created,
        }),
        parent_status_symbol: parent.status_symbol,
        parent_status_name: parent.status_name.clone(),
    };
    planner.stage(target, updated_target)?;
    Ok(CaptureWritePlan {
        placement,
        pomodoro: None,
        sub_bullet: Some(details),
        pomodoro_note: None,
        toggle: None,
        pomodoro_link: None,
        pomodoro_refs: Vec::new(),
    })
}

/// Capture into a project note's `##` section for a `^prj` parent: the
/// `#drawer` selector resolves against the note's eligible H2s by the drawer
/// key rules. A match appends the block after the section's last nonblank
/// line; a miss appends a new `##` section with the shared resolver's
/// create-title spelling at the end of the note. One staged write per item keeps batch
/// captures atomic: a later item re-resolves from staged text.
fn plan_prj_section_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    contents: &str,
    parent: &note_tasks::NoteTask,
    selector: &DrawerSelector,
    capture_block: &str,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<CaptureWritePlan, CaptureError> {
    // Load catalog for spelling (best-effort; empty on failure).
    let catalog =
        capture_drawers::load_drawer_catalog(bob_dir).unwrap_or_default();
    let drawers = capture_drawers::project_h2_drawers(contents);
    let resolved = match capture_drawers::resolve_h2_for_write(
        &drawers,
        &selector.text,
        selector.exact,
        &catalog,
        route,
        parent.block_id.as_deref(),
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            return Err(CaptureError::usage_code(
                error.message.clone(),
                error.code,
            ));
        }
    };
    if let Some(message) = resolved.warning.clone() {
        warnings.push(CaptureWarning::Structured(StructuredCaptureWarning {
            code: "drawer_near_catalog",
            label: None,
            count: None,
            message,
        }));
    }
    let (contents, drawer, created) =
        capture_drawers::ensure_drawer(contents, parent, &resolved)
            .map_err(CaptureError::io)?;
    prj_section_write_plan(
        planner,
        target,
        &contents,
        parent,
        drawer,
        created,
        capture_block,
    )
}

pub(crate) fn first_child_indentation(
    lines: &[LineSpan<'_>],
    parent_line_index: usize,
    block_end: usize,
    parent_indentation: &str,
) -> Option<String> {
    lines[parent_line_index + 1..]
        .iter()
        .take_while(|line| line.end <= block_end)
        .filter(|line| !line.text.trim().is_empty())
        .map(|line| leading_whitespace(line.text))
        .find(|indentation| indentation.len() > parent_indentation.len())
        .map(str::to_string)
}

// Plugin-compatible Schedule Log / Work Log / Rotten Log markers: optional
// matching emoji, the canonical or legacy bold label, an optional colon, and
// no trailing text. Direct-child ancestry skips blanks and uses the nearest
// shallower list item, so a log nested under another child does not move
// insertion.
// Parity note: the navigation-hotkeys-only `❌ **CANCEL LOG**` is
// deliberately NOT a managed-log anchor here. It is placed first under the
// task, so newly captured notes land below it via the managed-log anchors,
// and emoji-led bullets are never task drawers anyway. Mirrors
// `plugins/bob-navigation-hotkeys` managed-log grammar, which now also knows
// the Rotten kind.
pub(super) const SCHEDULE_LOG_EMOJI: &str = "🗓️";
pub(super) const WORK_LOG_EMOJI: &str = "🛠️";
pub(super) const ROTTEN_LOG_EMOJI: &str = "🥀";
pub(crate) const MANAGED_TASK_LOG_LABELS: &[(&str, ManagedTaskLogKind)] = &[
    ("SCHEDULE LOG", ManagedTaskLogKind::Schedule),
    ("Schedule log", ManagedTaskLogKind::Schedule),
    ("WORK LOG", ManagedTaskLogKind::Work),
    ("Work log", ManagedTaskLogKind::Work),
    ("ROTTEN LOG", ManagedTaskLogKind::Rotten),
    ("Rotten log", ManagedTaskLogKind::Rotten),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedTaskLogKind {
    Schedule,
    Work,
    Rotten,
}

pub(crate) fn first_direct_managed_log_start(
    lines: &[LineSpan<'_>],
    parent_line_index: usize,
    block_end: usize,
) -> Option<usize> {
    let mut line_start = lines[parent_line_index].end;
    for (offset, line) in lines[parent_line_index + 1..].iter().enumerate() {
        if line.end > block_end {
            break;
        }
        let line_index = parent_line_index + 1 + offset;
        if !line.text.trim().is_empty()
            && parse_managed_task_log_marker(line.text).is_some()
            && nearest_shallower_list_item_parent(lines, line_index)
                == Some(parent_line_index)
        {
            return Some(line_start);
        }
        line_start = line.end;
    }
    None
}

pub(crate) fn nearest_shallower_list_item_parent(
    lines: &[LineSpan<'_>],
    child_index: usize,
) -> Option<usize> {
    if child_index == 0 {
        return None;
    }
    let child_indent = leading_spaces_or_tabs_len(lines[child_index].text);
    for index in (0..child_index).rev() {
        let text = lines[index].text;
        if text.trim().is_empty()
            || leading_spaces_or_tabs_len(text) >= child_indent
        {
            continue;
        }
        if list_item_body(text).is_some() {
            return Some(index);
        }
    }
    None
}

pub(crate) fn parse_managed_task_log_marker(
    line: &str,
) -> Option<ManagedTaskLogKind> {
    let rest = list_item_body(line)?;
    if let Some(after_emoji) = strip_log_emoji(rest, SCHEDULE_LOG_EMOJI) {
        return parse_managed_task_log_label(after_emoji)
            .filter(|kind| *kind == ManagedTaskLogKind::Schedule);
    }
    if let Some(after_emoji) = strip_log_emoji(rest, WORK_LOG_EMOJI) {
        return parse_managed_task_log_label(after_emoji)
            .filter(|kind| *kind == ManagedTaskLogKind::Work);
    }
    if let Some(after_emoji) = strip_rotten_log_emoji(rest) {
        return parse_managed_task_log_label(after_emoji)
            .filter(|kind| *kind == ManagedTaskLogKind::Rotten);
    }
    parse_managed_task_log_label(rest)
}

/// `🥀` (U+1F940, no VS16) with an optional U+FE0F after it, then at least
/// one space or tab. A present emoji must agree with the label, so
/// `🗓️ **ROTTEN LOG**` is not a marker.
fn strip_rotten_log_emoji(rest: &str) -> Option<&str> {
    let after_emoji = rest.strip_prefix(ROTTEN_LOG_EMOJI)?;
    let after_emoji =
        after_emoji.strip_prefix('\u{FE0F}').unwrap_or(after_emoji);
    let whitespace = leading_spaces_or_tabs_len(after_emoji);
    (whitespace > 0).then_some(&after_emoji[whitespace..])
}

pub(super) fn parse_managed_task_log_label(
    rest: &str,
) -> Option<ManagedTaskLogKind> {
    let rest = rest.strip_prefix("**")?;
    for (label, kind) in MANAGED_TASK_LOG_LABELS {
        let Some(after_label) = rest.strip_prefix(label) else {
            continue;
        };
        let after_label = after_label.strip_prefix(':').unwrap_or(after_label);
        let Some(after_close) = after_label.strip_prefix("**") else {
            continue;
        };
        if after_close.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
            return Some(*kind);
        }
    }
    None
}

pub(super) fn strip_log_emoji<'a>(
    rest: &'a str,
    emoji: &str,
) -> Option<&'a str> {
    let after_emoji = rest.strip_prefix(emoji)?;
    let whitespace = leading_spaces_or_tabs_len(after_emoji);
    (whitespace > 0).then_some(&after_emoji[whitespace..])
}

pub(crate) fn list_item_body(line: &str) -> Option<&str> {
    let indent_len = leading_spaces_or_tabs_len(line);
    let after_indent = &line[indent_len..];
    let marker_len = list_marker_len(after_indent)?;
    let after_marker = &after_indent[marker_len..];
    let whitespace = leading_spaces_or_tabs_len(after_marker);
    (whitespace > 0).then_some(&after_marker[whitespace..])
}

pub(crate) fn list_marker_len(after_indent: &str) -> Option<usize> {
    let bytes = after_indent.as_bytes();
    match bytes.first() {
        Some(b'-' | b'*' | b'+') => Some(1),
        Some(b'0'..=b'9') => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            match bytes.get(digits) {
                Some(b'.' | b')') => Some(digits + 1),
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn leading_spaces_or_tabs_len(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

pub(crate) fn dominant_indent_unit(
    lines: &[LineSpan<'_>],
) -> Option<&'static str> {
    let (tabs, spaces) = lines.iter().fold((0usize, 0usize), |counts, line| {
        match line.text.as_bytes().first() {
            Some(b'\t') => (counts.0 + 1, counts.1),
            Some(b' ') => (counts.0, counts.1 + 1),
            _ => counts,
        }
    });
    if tabs + spaces == 0 {
        None
    } else if tabs > spaces {
        Some("\t")
    } else {
        Some("  ")
    }
}

pub(super) fn child_indent_unit(
    planner: &mut CaptureBatchPlanner,
    target: &Path,
) -> Result<String, CaptureError> {
    let Some(contents) = planner.current_contents(target)? else {
        return Ok("\t".to_string());
    };
    Ok(dominant_indent_unit(&line_spans(&contents))
        .unwrap_or("\t")
        .to_string())
}

pub(crate) fn leading_whitespace(line: &str) -> &str {
    let end = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    &line[..end]
}
