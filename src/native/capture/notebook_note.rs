//! Notebook note planning: section resolution and guarded note writes.

use super::*;
use crate::native::capture_language::notebook::{
    NotebookRouteInfo, NotebookRoutePolicy,
};
use crate::native::projects::{frontmatter_is_notebook, parse_frontmatter};

/// Machine-readable notebook summary carried on each dry-run/success item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NotebookSummary {
    pub(crate) route: String,
    pub(crate) title: String,
    pub(crate) breadcrumb: Vec<String>,
    pub(crate) section: Option<String>,
    pub(crate) section_action: Option<&'static str>,
    pub(crate) block_id: Option<String>,
    pub(crate) parent_block_id: Option<String>,
    /// Planned create file collected into top-level `note_files`. Omitted
    /// on ordinary notebook notes so existing JSON stays byte-identical.
    #[serde(skip)]
    pub(crate) note_file: Option<super::output::NoteFileJson>,
}

fn line_ending_of(contents: &str) -> &'static str {
    sections::document_line_ending(contents)
}

fn split_lines_keep(contents: &str) -> Vec<String> {
    if contents.is_empty() {
        return Vec::new();
    }
    // Preserve content exactly: split on `\n`, keeping `\r` handling to
    // the join step via the document line ending.
    contents.split('\n').map(str::to_string).collect()
}

fn join_lines(
    lines: &[String],
    ending: &str,
    trailing_newline: bool,
) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        out.push_str(line);
        if index + 1 < lines.len() || trailing_newline {
            // Existing rows keep their own terminator. Newly inserted rows
            // use the document's detected line ending.
            if line.ends_with('\r') {
                out.push('\n');
            } else {
                out.push_str(ending);
            }
        }
    }
    out
}

/// Byte-offset-free line helpers: leading indent width in chars.
fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .count()
}

fn is_top_level_bullet(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    trimmed.starts_with("- ") || trimmed == "-" || trimmed.starts_with("-\t")
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn logical_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

fn fenced_line_indices(lines: &[String]) -> std::collections::BTreeSet<usize> {
    let logical = logical_lines(lines);
    let borrowed = logical.iter().map(String::as_str).collect::<Vec<_>>();
    markdown::fenced_lines(&borrowed, 0..borrowed.len())
}

fn real_heading_levels(lines: &[String]) -> Vec<(usize, usize)> {
    let logical = logical_lines(lines).join("\n");
    let spans = sections::line_spans(&logical);
    sections::markdown_headings(&spans)
        .into_iter()
        .map(|heading| (heading.line_index, heading.level))
        .collect()
}

fn mask_inline_code_and_wikilinks(line: &str) -> String {
    let mut chars = line.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '`' {
            let run_end = (index..chars.len())
                .take_while(|position| chars[*position] == '`')
                .last()
                .map_or(index + 1, |position| position + 1);
            let run = run_end - index;
            let close = (run_end..chars.len()).find(|position| {
                if chars[*position] != '`' {
                    return false;
                }
                let end = (*position..chars.len())
                    .take_while(|candidate| chars[*candidate] == '`')
                    .last()
                    .map_or(*position + 1, |candidate| candidate + 1);
                end - *position == run
            });
            if let Some(close) = close {
                let close_end = (close..chars.len())
                    .take_while(|position| chars[*position] == '`')
                    .last()
                    .map_or(close + 1, |position| position + 1);
                for character in &mut chars[index..close_end] {
                    *character = ' ';
                }
                index = close_end;
                continue;
            }
            index = run_end;
            continue;
        }
        if chars[index] == '['
            && chars.get(index + 1) == Some(&'[')
            && (index == 0 || chars[index - 1] != '\\')
            && let Some(close) = (index + 2..chars.len().saturating_sub(1))
                .find(|position| {
                    chars[*position] == ']' && chars[*position + 1] == ']'
                })
        {
            for character in &mut chars[index..close + 2] {
                *character = ' ';
            }
            index = close + 2;
            continue;
        }
        index += 1;
    }
    chars.into_iter().collect()
}

/// Actual trailing block anchors across Markdown blocks. Inline code,
/// wikilink destinations, frontmatter, and fenced examples are prose.
fn block_ids_outside_fences(contents: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = contents
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = markdown::fenced_lines(&lines, scan_start..lines.len());
    lines
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            frontmatter_end.is_none_or(|end| *index > end)
                && !fenced.contains(index)
        })
        .filter_map(|(index, line)| {
            let visible = mask_inline_code_and_wikilinks(line);
            crate::native::collect_done::trailing_block_id_in_line(&visible)
                .map(|id| (id, index))
        })
        .collect()
}

fn is_markdown_bullet(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ")
        || trimmed.starts_with("-\t")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("*\t")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("+\t")
    {
        return true;
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && trimmed.as_bytes().get(digits).is_some_and(|byte| {
            matches!(byte, b'.' | b')')
                && trimmed
                    .as_bytes()
                    .get(digits + 1)
                    .is_some_and(u8::is_ascii_whitespace)
        })
}

fn parent_indent_of(line: &str) -> String {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect()
}

/// Find the parent note bullet for `@nb+id` at any depth, skipping fenced
/// examples. Returns `(line_index, indent)`.
fn find_notebook_parent(
    contents: &str,
    parent_block_id: &str,
    route: &str,
) -> Result<(usize, String), CaptureError> {
    let raw_lines: Vec<&str> = contents.split('\n').collect();
    let fenced = markdown::fenced_lines(&raw_lines, 0..raw_lines.len());
    let occurrences = block_ids_outside_fences(contents)
        .into_iter()
        .filter(|(id, _)| id == parent_block_id)
        .map(|(_, index)| index)
        .collect::<Vec<_>>();
    if occurrences.len() > 1 {
        return Err(CaptureError::io(format!(
            "block ID ^{parent_block_id} already exists in {route}.md"
        )));
    }
    let mut fenced_match = false;
    for (index, line) in raw_lines.iter().enumerate() {
        if !fenced.contains(&index) {
            continue;
        }
        let logical = line.strip_suffix('\r').unwrap_or(line);
        let visible = mask_inline_code_and_wikilinks(logical);
        if crate::native::collect_done::trailing_block_id_in_line(&visible)
            .as_deref()
            == Some(parent_block_id)
        {
            fenced_match = true;
            break;
        }
    }
    let lines = raw_lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let headings = real_heading_levels(&lines)
        .into_iter()
        .map(|(index, _)| index)
        .collect::<std::collections::HashSet<_>>();
    let heading_match = occurrences
        .first()
        .is_some_and(|index| headings.contains(index));
    let task_match = occurrences.first().is_some_and(|index| {
        crate::native::notebooks::rules::is_checkbox_line(
            raw_lines[*index]
                .strip_suffix('\r')
                .unwrap_or(raw_lines[*index]),
        )
    });
    if let Some(index) = occurrences.first().copied()
        && !heading_match
        && !task_match
        && is_markdown_bullet(
            raw_lines[index]
                .strip_suffix('\r')
                .unwrap_or(raw_lines[index]),
        )
    {
        let indent = parent_indent_of(raw_lines[index]);
        return Ok((index, indent));
    }
    // No bullet parent: report the most useful existing-target error.
    if fenced_match {
        return Err(CaptureError::usage_code(
            format!(
                "no note ^{parent_block_id} in {route}.md: that ID lives inside a fenced example"
            ),
            "notebook_parent_not_found",
        ));
    }
    if heading_match {
        return Err(CaptureError::usage_code(
            format!(
                "no note ^{parent_block_id} in {route}.md: headings cannot parent notes"
            ),
            "notebook_parent_not_found",
        ));
    }
    if task_match {
        return Err(CaptureError::usage_code(
            format!(
                "no note ^{parent_block_id} in {route}.md: tasks cannot parent notebook notes — use @{route}"
            ),
            "notebook_parent_not_found",
        ));
    }
    // Missing with a did-you-mean suggestion over live IDs.
    let live: Vec<String> = block_ids_outside_fences(contents)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let mut best: Option<(usize, String)> = None;
    for id in &live {
        if let Some(distance) =
            note_tasks::bounded_levenshtein(parent_block_id, id, 2)
        {
            let better = best
                .as_ref()
                .is_none_or(|(best_dist, _)| distance < *best_dist);
            if better {
                best = Some((distance, id.clone()));
            }
        }
    }
    if let Some((_, suggestion)) = best {
        return Err(CaptureError::usage_code(
            format!(
                "no note ^{parent_block_id} in {route}.md — did you mean ^{suggestion}?"
            ),
            "notebook_parent_not_found",
        ));
    }
    Err(CaptureError::usage_code(
        format!("no note ^{parent_block_id} in {route}.md"),
        "notebook_parent_not_found",
    ))
}

/// End of a bullet's nested block: trailing blank lines belong to the
/// parent, while the next heading, a same-or-lower indent bullet, or EOF
/// ends it.
fn bullet_block_end(
    lines: &[String],
    parent_index: usize,
    parent_indent: usize,
    fenced: &std::collections::BTreeSet<usize>,
) -> usize {
    let mut end = parent_index + 1;
    let mut last_content = parent_index + 1;
    while end < lines.len() {
        let line = &lines[end];
        let logical = line.strip_suffix('\r').unwrap_or(line);
        if fenced.contains(&end) {
            last_content = end + 1;
            end += 1;
            continue;
        }
        if is_blank(logical) {
            end += 1;
            continue;
        }
        let trimmed = logical.trim_start();
        if trimmed.starts_with('#') && !logical.starts_with([' ', '\t']) {
            break;
        }
        let width = indent_width(logical);
        if is_top_level_bullet(logical) && width <= parent_indent {
            break;
        }
        if width <= parent_indent && !logical.starts_with([' ', '\t']) {
            break;
        }
        last_content = end + 1;
        end += 1;
    }
    last_content
}

fn region_bounds(
    lines: &[String],
    heading_line: Option<usize>,
) -> (usize, usize) {
    let headings = real_heading_levels(lines);
    if let Some(heading) = heading_line {
        let end = headings
            .iter()
            .find(|(line, level)| *line > heading && *level <= 2)
            .map(|(line, _)| *line)
            .unwrap_or(lines.len());
        (heading + 1, end)
    } else {
        // The pre-H2 region retains YAML, the early H1, and preamble.
        let end = headings
            .iter()
            .find(|(_, level)| *level == 2)
            .map(|(line, _)| *line)
            .unwrap_or(lines.len());
        (0, end)
    }
}

fn last_note_block_end_in_region(
    lines: &[String],
    start: usize,
    end: usize,
) -> Option<usize> {
    let mut last: Option<usize> = None;
    let mut index = start;
    let fenced = fenced_line_indices(lines);
    while index < end {
        let logical = lines[index].strip_suffix('\r').unwrap_or(&lines[index]);
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        if is_blank(logical) {
            index += 1;
            continue;
        }
        if is_top_level_bullet(logical) {
            let block_end = bullet_block_end(lines, index, 0, &fenced);
            last = Some(block_end.min(end));
            index = block_end;
        } else {
            index += 1;
        }
    }
    last
}

/// Render authored child bullets verbatim beneath a note, without task or
/// drawer interpretation.
fn render_notebook_children(
    sub_bullets: &[AuthoredSubBullet],
    indent_unit: &str,
) -> Vec<String> {
    sub_bullets
        .iter()
        .map(|item| {
            let indentation = indent_unit.repeat(item.depth.indent_units());
            format!("{indentation}- {}", item.body)
        })
        .collect()
}

fn clip_lines_for_note(
    request: &CaptureRequest,
    parsed_clip: Option<&ClipRequest>,
    clip_snapshot: &mut plan::ClipSnapshot,
    clip_reservations: &mut capture_clip::ClipReservations,
    now: chrono::NaiveDateTime,
    indent: &str,
) -> Result<Option<capture_clip::ClipPlan>, CaptureError> {
    let effective = parsed_clip.or(request.forced_clip.as_ref());
    let Some(clip) = effective else {
        return Ok(None);
    };
    if request.no_clip {
        if request.dry_run {
            return Ok(Some(capture_clip::ClipPlan::not_read(clip.count())));
        }
        return Err(CaptureError::usage_code(
            capture_language::clip_not_read_error(),
            "clip_not_read",
        ));
    }
    match clip {
        ClipRequest::Current => {
            let read = plan::read_capture_clipboard(clip_snapshot, clip)?;
            let plan::FanoutClipboardRead::Current(clipboard) = read else {
                return Err(CaptureError::io(
                    "clipboard capture invariant failed: current clipboard read expected",
                ));
            };
            Ok(Some(
                capture_clip::plan_with_reservations(
                    &request.bob_dir,
                    &clipboard,
                    now,
                    indent,
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            ))
        }
        ClipRequest::History { .. } => {
            let read = plan::read_capture_clipboard(clip_snapshot, clip)?;
            let plan::FanoutClipboardRead::History(clipboards) = read else {
                return Err(CaptureError::io(
                    "clipboard capture invariant failed: history clipboard read expected",
                ));
            };
            Ok(Some(
                capture_clip::plan_history_with_reservations(
                    &request.bob_dir,
                    &clipboards,
                    now,
                    indent,
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_notebook_note_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    policy: &NotebookRoutePolicy,
    clip_reservations: &mut capture_clip::ClipReservations,
    clip_snapshot: &mut plan::ClipSnapshot,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    let parsed = parsed_item.parsed;
    let route = parsed.route.clone().ok_or_else(|| {
        CaptureError::io("notebook capture invariant failed: route is missing")
    })?;
    let info = policy.get(&route).ok_or_else(|| {
        CaptureError::io(format!(
            "notebook capture invariant failed: @{route} is not a notebook"
        ))
    })?;
    let relative = plan::relative_target(Some(&route));
    let target = request.bob_dir.join(&relative);
    // Staged bytes win so later items see earlier notes; the first read
    // records the disk preimage for the commit-time drift check.
    let contents = planner.read_existing(&target)?;
    // Policy/planner preimage: classification read these bytes, so a
    // retype between classification and planning aborts the whole draft.
    // Compare against the planner's original disk snapshot, not the staged
    // current bytes, so later items in the same draft (which see earlier
    // staged notes) do not trip the guard.
    if let Some(expected) = policy.source(&route)
        && let Some((original, _)) = planner.loaded_texts(&target)
        && expected != original
    {
        return Err(CaptureError::io(format!(
            "refusing to overwrite {}: note changed on disk after planning",
            target.display()
        ))
        .with_typed(CaptureFailureKind::NoteChanged));
    }
    // A retype that drops the notebook frontmatter keeps the route but
    // must never write a note into it.
    let is_notebook = parse_frontmatter(&contents)
        .is_some_and(|frontmatter| frontmatter_is_notebook(&frontmatter));
    if !is_notebook {
        return Err(CaptureError::usage(format!(
            "{route}.md is no longer a notebook; run `bob notebooks check`"
        )));
    }
    let created = plan::date_string(today);
    let ending = line_ending_of(&contents);
    match parsed.kind.clone() {
        CaptureKind::NotebookNote {
            section,
            exact_section,
            block_id,
        } => plan_notebook_note(
            request,
            parsed,
            route,
            info,
            section,
            exact_section,
            block_id,
            created,
            ending,
            contents,
            now,
            planner,
            clip_reservations,
            clip_snapshot,
            warnings,
            &target,
            &relative,
        ),
        CaptureKind::NotebookSubNote { parent_block_id } => {
            plan_notebook_sub_note(
                request,
                parsed,
                route,
                info,
                parent_block_id,
                ending,
                contents,
                now,
                planner,
                clip_reservations,
                clip_snapshot,
                &target,
                &relative,
            )
        }
        _ => Err(CaptureError::io(
            "notebook-note capture invariant failed: wrong capture kind",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_notebook_note(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    route: String,
    info: &NotebookRouteInfo,
    section: Option<String>,
    exact_section: bool,
    block_id: Option<String>,
    created: String,
    ending: &'static str,
    contents: String,
    now: chrono::NaiveDateTime,
    planner: &mut CaptureBatchPlanner,
    clip_reservations: &mut capture_clip::ClipReservations,
    clip_snapshot: &mut plan::ClipSnapshot,
    warnings: &mut Vec<CaptureWarning>,
    target: &Path,
    relative: &Path,
) -> Result<PlannedCaptureItem, CaptureError> {
    let (section_title, section_action, mut heading_line, create_title) =
        match section.as_deref() {
            None => (None, None, None, None),
            Some(selector) => {
                match capture_language::resolve_notebook_section(
                    selector,
                    exact_section,
                    &capture_language::notebook_sections(&contents),
                ) {
                    capture_language::NotebookSectionResolution::Existing {
                        title,
                        line_index,
                    } => {
                        (Some(title), Some("existing"), Some(line_index), None)
                    }
                    capture_language::NotebookSectionResolution::Create {
                        title,
                    } => {
                        (Some(title.clone()), Some("create"), None, Some(title))
                    }
                    capture_language::NotebookSectionResolution::NearMiss {
                        slug,
                        quoted_title,
                    } => {
                        return Err(CaptureError::usage_code(
                            capture_language::notebook_section_near_miss_message(
                                &route,
                                selector,
                                &slug,
                                &quoted_title,
                            ),
                            "notebook_section_near_miss",
                        ));
                    }
                }
            }
        };
    // New-section creation appends the heading at EOF first.
    let mut lines = split_lines_keep(&contents);
    // `split('\n')` on an empty file yields `[""]`; normalize to empty so
    // insertion does not emit a stray blank line.
    if lines.len() == 1 && lines[0].is_empty() {
        lines.clear();
    }
    let had_trailing_newline = contents.ends_with('\n');
    // Drop the synthetic trailing element from the split so indexes line
    // up with real lines; it is restored on join.
    if had_trailing_newline
        && !lines.is_empty()
        && lines.last().is_some_and(|l| l.is_empty())
    {
        lines.pop();
    }
    if section_action == Some("create") {
        let title = create_title.unwrap_or_default();
        if !lines.is_empty() {
            let last = lines.last().map(String::as_str).unwrap_or("");
            if !is_blank(last.strip_suffix('\r').unwrap_or(last)) {
                lines.push(String::new());
            }
        }
        lines.push(format!("## {title}"));
        lines.push(String::new());
        heading_line = Some(lines.len() - 2);
    }
    let (region_start, region_end) = region_bounds(&lines, heading_line);
    let insert_at =
        last_note_block_end_in_region(&lines, region_start, region_end)
            .unwrap_or(region_end);
    // Note line through the shared bullet formatter, plus the named ID.
    let mut note_line =
        plan::format_bullet_line(&parsed.body, &created, None, None);
    if let Some(block_id) = block_id.as_deref() {
        plan::append_block_id(&mut note_line, block_id);
    }
    let child_indent = super::sub_bullet::child_indent_unit(planner, target)
        .unwrap_or_else(|_| "\t".to_string());
    let authored_children =
        render_notebook_children(&parsed.sub_bullets, &child_indent);
    let clip_plan = clip_lines_for_note(
        request,
        parsed.clip.as_ref(),
        clip_snapshot,
        clip_reservations,
        now,
        &child_indent,
    )?;
    let mut clip_output = None;
    if let Some(plan) = clip_plan.as_ref() {
        clip_output = Some(plan.output.clone());
    }
    let mut block: Vec<String> = vec![note_line.clone()];
    for rendered in &authored_children {
        block.push(rendered.clone());
    }
    if let Some(plan) = clip_plan.as_ref() {
        for line in &plan.output.lines {
            block.push(line.clone());
        }
    }
    let block_text = block.join("\n");
    if crate::native::notebooks::rules::contains_checkbox_rows(&block_text) {
        return Err(CaptureError::usage_code(
            capture_language::notebook_holds_no_tasks_error(
                &route,
                info.owner.as_deref(),
            ),
            "notebook_holds_no_tasks",
        ));
    }
    // Duplicate IDs across earlier writes in the same draft: the staged
    // `contents` already include them, and the new block must not repeat
    // any live ID either.
    let live_ids: std::collections::HashSet<String> =
        block_ids_outside_fences(&contents)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
    let mut seen_new: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (id, _) in block_ids_outside_fences(&block_text) {
        if live_ids.contains(&id) || !seen_new.insert(id.clone()) {
            return Err(CaptureError::io(format!(
                "block ID ^{id} already exists in {}",
                target.display()
            )));
        }
    }
    // Warn when clipboard history fell back, matching task-clip behavior.
    let _ = warnings;
    let mut staged = lines.clone();
    // Insert with a blank separator above when needed; the block itself
    // carries no trailing blank (the region's existing blanks stay).
    let mut at = insert_at;
    if at > region_start {
        let prev = staged.get(at - 1).map(String::as_str).unwrap_or("");
        let prev_logical = prev.strip_suffix('\r').unwrap_or(prev);
        if !is_blank(prev_logical) {
            staged.insert(at, String::new());
            at += 1;
        }
    } else if heading_line.is_some() {
        // First note in a section: the heading is followed by exactly one
        // blank line already (creation) or existing blanks; ensure one.
        if staged
            .get(at)
            .is_some_and(|l| !is_blank(l.strip_suffix('\r').unwrap_or(l)))
        {
            staged.insert(at, String::new());
        }
    }
    for (offset, line) in block.iter().enumerate() {
        staged.insert(at + offset, line.clone());
    }
    // Blank separator below when the next line is a non-blank heading or
    // note (notably the `##` that ends a pre-H2 insertion).
    if let Some(next) = staged.get(at + block.len()).map(String::as_str) {
        let next_logical = next.strip_suffix('\r').unwrap_or(next);
        if !is_blank(next_logical) {
            staged.insert(at + block.len(), String::new());
        }
    }
    let updated = join_lines(&staged, ending, had_trailing_newline);
    planner.stage(target, updated)?;
    let relative_str = relative.to_string_lossy().replace('\\', "/");
    let summary = NotebookSummary {
        route: route.clone(),
        title: info.title.clone(),
        breadcrumb: info.breadcrumb.clone(),
        section: section_title.clone(),
        section_action,
        block_id: block_id.clone(),
        parent_block_id: None,
        note_file: None,
    };
    let task_line = block.first().cloned().unwrap_or_default();
    let sub_bullets = authored_children.clone();
    Ok(PlannedCaptureItem {
        result: notebook_item_result(
            request,
            &route,
            &relative_str,
            target,
            &parsed.body,
            &task_line,
            sub_bullets,
            clip_output,
            summary,
            "notebook_note",
        ),
        clip_plan,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
        project_move_landings: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn plan_notebook_sub_note(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    route: String,
    info: &NotebookRouteInfo,
    parent_block_id: String,
    ending: &'static str,
    contents: String,
    now: chrono::NaiveDateTime,
    planner: &mut CaptureBatchPlanner,
    clip_reservations: &mut capture_clip::ClipReservations,
    clip_snapshot: &mut plan::ClipSnapshot,
    target: &Path,
    relative: &Path,
) -> Result<PlannedCaptureItem, CaptureError> {
    let (parent_line, parent_indent) =
        find_notebook_parent(&contents, &parent_block_id, &route)?;
    let mut lines = split_lines_keep(&contents);
    if lines.len() == 1 && lines[0].is_empty() {
        lines.clear();
    }
    let had_trailing_newline = contents.ends_with('\n');
    if had_trailing_newline
        && !lines.is_empty()
        && lines.last().is_some_and(|l| l.is_empty())
    {
        lines.pop();
    }
    // Child indent: parent indent plus the file's dominant unit.
    let unit = super::sub_bullet::child_indent_unit(planner, target)
        .unwrap_or_else(|_| "\t".to_string());
    let child_indent = format!("{parent_indent}{unit}");
    let detail_indent = format!("{child_indent}{unit}");
    let authored_children: Vec<String> =
        render_notebook_children(&parsed.sub_bullets, &unit)
            .into_iter()
            .map(|line| format!("{child_indent}{line}"))
            .collect();
    let clip_plan = clip_lines_for_note(
        request,
        parsed.clip.as_ref(),
        clip_snapshot,
        clip_reservations,
        now,
        &detail_indent,
    )?;
    let mut clip_output = None;
    if let Some(plan) = clip_plan.as_ref() {
        clip_output = Some(plan.output.clone());
    }
    let mut block: Vec<String> =
        vec![format!("{child_indent}- {}", parsed.body)];
    for rendered in &authored_children {
        block.push(rendered.clone());
    }
    if let Some(plan) = clip_plan.as_ref() {
        for line in &plan.output.lines {
            // Clipboard lines already carry the requested indent; nest
            // them under the parent when they do not.
            if line.starts_with(&child_indent) {
                block.push(line.clone());
            } else {
                block.push(format!("{detail_indent}{}", line.trim_start()));
            }
        }
    }
    let block_text = block.join("\n");
    if crate::native::notebooks::rules::contains_checkbox_rows(&block_text) {
        return Err(CaptureError::usage_code(
            capture_language::notebook_holds_no_tasks_error(
                &route,
                info.owner.as_deref(),
            ),
            "notebook_holds_no_tasks",
        ));
    }
    // Duplicate check for any new IDs (terminal IDs on sub-note bodies
    // count, even though the kind carries no block_id).
    let live_ids: std::collections::HashSet<String> =
        block_ids_outside_fences(&contents)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
    let mut seen_new: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (id, _) in block_ids_outside_fences(&block_text) {
        if live_ids.contains(&id) || !seen_new.insert(id.clone()) {
            return Err(CaptureError::io(format!(
                "block ID ^{id} already exists in {}",
                target.display()
            )));
        }
    }
    let fenced = fenced_line_indices(&lines);
    let insert_at = bullet_block_end(
        &lines,
        parent_line,
        indent_width(&lines[parent_line]),
        &fenced,
    );
    let mut staged = lines.clone();
    for (offset, line) in block.iter().enumerate() {
        staged.insert(insert_at + offset, line.clone());
    }
    let updated = join_lines(&staged, ending, had_trailing_newline);
    planner.stage(target, updated)?;
    let relative_str = relative.to_string_lossy().replace('\\', "/");
    let summary = NotebookSummary {
        route: route.clone(),
        title: info.title.clone(),
        breadcrumb: info.breadcrumb.clone(),
        section: None,
        section_action: None,
        block_id: None,
        parent_block_id: Some(parent_block_id.clone()),
        note_file: None,
    };
    let task_line = block.first().cloned().unwrap_or_default();
    let sub_bullets = authored_children.clone();
    Ok(PlannedCaptureItem {
        result: notebook_item_result(
            request,
            &route,
            &relative_str,
            target,
            &parsed.body,
            &task_line,
            sub_bullets,
            clip_output,
            summary,
            "notebook_sub_note",
        ),
        clip_plan,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
        project_move_landings: Vec::new(),
    })
}

pub(super) fn notebook_item_result(
    request: &CaptureRequest,
    route: &str,
    relative_str: &str,
    target: &Path,
    body: &str,
    task_line: &str,
    sub_bullets: Vec<String>,
    clip: Option<capture_clip::ClipOutput>,
    notebook: NotebookSummary,
    kind: &'static str,
) -> crate::native::capture::output::CaptureItemResult {
    crate::native::capture::output::CaptureItemResult {
        ok: true,
        dry_run: request.dry_run,
        routed: true,
        route: Some(route.to_string()),
        route_label: plan::route_label(route),
        relative_target: relative_str.to_string(),
        target: target.display().to_string(),
        text: body.to_string(),
        task_line: task_line.to_string(),
        task_theme: None,
        type_relocation: None,
        kind,
        fanout: None,
        created: String::new(),
        scheduled: None,
        priority: None,
        priority_label: None,
        placement: crate::native::capture::Placement::Appended,
        sub_bullets,
        notes: Vec::new(),
        drawers: Vec::new(),
        clip,
        schedule_log: None,
        block_id: notebook
            .block_id
            .clone()
            .or_else(|| notebook.parent_block_id.clone()),
        day_file: None,
        block_link: None,
        pomodoro_link_placement: None,
        parent_line: None,
        parent_text: None,
        drawer: None,
        parent_status_symbol: None,
        parent_status_name: None,
        toggle_direction: None,
        previous_task_line: None,
        status_symbol: None,
        status_name: None,
        previous_status_symbol: None,
        previous_status_name: None,
        pomodoro_name: None,
        creates_pomodoro: None,
        pomodoro_already_linked: None,
        removed_pomodoro_links: None,
        removed_scheduled: None,
        pomodoro_selector_unused: None,
        toggle_behavior: None,
        status_changed: None,
        pomodoro_link_action: None,
        pomodoro_link_source: None,
        pomodoro_link_destination: None,
        project_note: None,
        pomodoro_start: None,
        pomodoro_adjust: None,
        pomodoro_shift: None,
        pomodoro_split: None,
        pomodoro_merge: None,
        pomodoro_close: None,
        pomodoro_past: None,
        pomodoro_reset: None,
        dependency_update: None,
        consumed_links: Vec::new(),
        task_complete: None,
        r#ref: None,
        gmail: Vec::new(),
        toggle_task_description: None,
        link_guard: None,
        dropped_dependencies: Vec::new(),
        blocks_line_updates: Vec::new(),
        minted_dependent_block_id: None,
        dependency_updates: Vec::new(),
        minted_blocker_block_id: None,
        relation_updates: Vec::new(),
        relation_cancels: Vec::new(),
        relation_completes: Vec::new(),
        relation_merge_output: Default::default(),
        transfer_move_output: Default::default(),
        relation_label_hides: Vec::new(),
        linked_from_line_updates: Vec::new(),
        minted_relation_block_ids: Vec::new(),
        mirror_skipped: Vec::new(),
        list: None,
        relation_successors: Vec::new(),
        relation_held: Vec::new(),
        relation_check: None,
        relation_groups: Vec::new(),
        note_pull: None,
        record_note: None,
        notebook: Some(notebook),
    }
}
