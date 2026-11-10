//! Notebook note planning: section resolution and guarded note writes.

use super::*;
use crate::native::capture_language::notebook::{
    NotebookRouteInfo, NotebookRoutePolicy,
};
use crate::native::capture_project_note::format_project_section_title;
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
}

struct SectionResolution {
    /// Resolved heading title (`None` for the pre-H2 region).
    title: Option<String>,
    /// `existing`, `create`, or `None` for the pre-H2 region.
    action: Option<&'static str>,
    /// Line index of the resolved `##` heading, when existing.
    heading_line: Option<usize>,
    /// Title to create when `action == Some("create")`.
    create_title: Option<String>,
}

struct NotebookHeadings {
    /// `(line_index, title)` for every `##` heading in document order.
    headings: Vec<(usize, String)>,
}

fn notebook_h2_headings(contents: &str) -> NotebookHeadings {
    let lines = sections::line_spans(contents);
    let headings = sections::markdown_headings(&lines);
    let headings = headings
        .into_iter()
        .filter(|heading| heading.level == 2)
        .map(|heading| (heading.line_index, heading.title.to_string()))
        .collect();
    NotebookHeadings { headings }
}

fn slug_of(text: &str) -> String {
    capture_language::selector_slug(text)
}

/// Resolve a notebook section selector against the file's `##` headings.
///
/// Exact selectors (quoted or forced `--section`) match whole-title only.
/// Unquoted selectors match exact title or whole slug, refuse near misses,
/// and otherwise create a Title Case `##`.
fn resolve_notebook_section(
    contents: &str,
    selector: Option<&str>,
    exact: bool,
    route: &str,
) -> Result<SectionResolution, CaptureError> {
    let Some(selector) = selector else {
        return Ok(SectionResolution {
            title: None,
            action: None,
            heading_line: None,
            create_title: None,
        });
    };
    let headings = notebook_h2_headings(contents);
    if exact {
        for (line_index, title) in &headings.headings {
            if title.to_lowercase() == selector.to_lowercase() {
                return Ok(SectionResolution {
                    title: Some(title.clone()),
                    action: Some("existing"),
                    heading_line: Some(*line_index),
                    create_title: None,
                });
            }
        }
        return Ok(SectionResolution {
            title: Some(selector.to_string()),
            action: Some("create"),
            heading_line: None,
            create_title: Some(selector.to_string()),
        });
    }
    // Unquoted: exact title or whole slug wins.
    for (line_index, title) in &headings.headings {
        if title.to_lowercase() == selector.to_lowercase()
            || (!slug_of(title).is_empty()
                && slug_of(title) == slug_of(selector))
        {
            return Ok(SectionResolution {
                title: Some(title.clone()),
                action: Some("existing"),
                heading_line: Some(*line_index),
                create_title: None,
            });
        }
    }
    // Near miss: selector prefixes an existing slug, or is within edit
    // distance two of one.
    let query_slug = slug_of(selector);
    let mut suggestion: Option<(String, String)> = None;
    for (_, title) in &headings.headings {
        let existing_slug = slug_of(title);
        if existing_slug.is_empty() || query_slug.is_empty() {
            continue;
        }
        if existing_slug.starts_with(query_slug.as_str()) {
            suggestion = Some((existing_slug, title.clone()));
            break;
        }
    }
    if suggestion.is_none() && !query_slug.is_empty() {
        let mut best: Option<(usize, String, String)> = None;
        for (_, title) in &headings.headings {
            let existing_slug = slug_of(title);
            if existing_slug.is_empty() {
                continue;
            }
            if let Some(distance) =
                note_tasks::bounded_levenshtein(&query_slug, &existing_slug, 2)
            {
                let better = best
                    .as_ref()
                    .is_none_or(|(best_dist, _, _)| distance < *best_dist);
                if better {
                    best =
                        Some((distance, existing_slug.clone(), title.clone()));
                }
            }
        }
        if let Some((_, slug, _)) = best {
            let title = headings
                .headings
                .iter()
                .find(|(_, t)| slug_of(t) == slug)
                .map(|(_, t)| t.clone())
                .unwrap_or(slug.clone());
            suggestion = Some((slug, title));
        }
    }
    if let Some((slug, _)) = suggestion {
        let file = format!("{route}.md");
        let quoted = format_project_section_title(&selector.replace('-', " "));
        return Err(CaptureError::usage_code(
            format!(
                "no section `{selector}` in {file} — did you mean `#{slug}`? Write `#\"{quoted}\"` to create it"
            ),
            "notebook_section_near_miss",
        ));
    }
    let create_title =
        format_project_section_title(&selector.replace('-', " "));
    Ok(SectionResolution {
        title: Some(create_title.clone()),
        action: Some("create"),
        heading_line: None,
        create_title: Some(create_title),
    })
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

fn join_lines(lines: &[String], ending: &str) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join(ending);
    // `split('\n')` on a trailing-newline file yields a final empty
    // element; joining restores the trailing ending exactly.
    let _ = ending;
    out.push('\n');
    // Normalize a lone `\n` join to CRLF when the document is CRLF.
    if ending == "\r\n" && out.ends_with('\n') && !out.ends_with("\r\n") {
        out.pop();
        out.push_str("\r\n");
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

/// Collect every `^id` in staged bytes outside fenced code blocks.
fn block_ids_outside_fences(contents: &str) -> Vec<(String, usize)> {
    let raw_lines: Vec<&str> = contents.split('\n').collect();
    let fenced = markdown::fenced_lines(&raw_lines, 0..raw_lines.len());
    let mut ids = Vec::new();
    for (index, line) in raw_lines.iter().enumerate() {
        if fenced.contains(&index) {
            continue;
        }
        let logical = sections::logical_line(line);
        for token in logical.split_whitespace() {
            if let Some(id) = token.strip_prefix('^')
                && !id.is_empty()
                && capture_language::is_block_id(id)
            {
                // Strip trailing punctuation that cannot be part of an ID.
                let clean = id
                    .trim_end_matches([',', '.', ';', ':', '!', '?', ')', ']']);
                if !clean.is_empty() && capture_language::is_block_id(clean) {
                    ids.push((clean.to_string(), index));
                }
            }
        }
    }
    ids
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
    let mut matches: Vec<usize> = Vec::new();
    let mut fenced_match = false;
    let mut heading_match = false;
    let mut task_match = false;
    for (index, line) in raw_lines.iter().enumerate() {
        let logical = sections::logical_line(line);
        let has_id = logical.split_whitespace().any(|token| {
            token.strip_prefix('^').is_some_and(|id| {
                let clean = id
                    .trim_end_matches([',', '.', ';', ':', '!', '?', ')', ']']);
                clean == parent_block_id
            })
        });
        if !has_id {
            continue;
        }
        if fenced.contains(&index) {
            fenced_match = true;
            continue;
        }
        let trimmed = logical.trim_start();
        if trimmed.starts_with('#') {
            heading_match = true;
            continue;
        }
        // Task lines hold `#task` or a checkbox; they can never parent a
        // notebook sub-note.
        if trimmed.starts_with("- [") || logical.contains("#task") {
            task_match = true;
            continue;
        }
        if is_top_level_bullet(logical)
            || logical.starts_with([' ', '\t'])
                && logical.trim_start().starts_with("- ")
            || logical.trim_start().starts_with("-\t")
            || logical.trim_start() == "-"
        {
            matches.push(index);
        } else {
            // A bare `^id` outside a bullet is not a note parent.
            continue;
        }
    }
    if matches.len() > 1 {
        return Err(CaptureError::io(format!(
            "block ID ^{parent_block_id} already exists in {route}.md"
        )));
    }
    if let Some(index) = matches.first().copied() {
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
) -> usize {
    let mut end = parent_index + 1;
    let mut last_content = parent_index + 1;
    while end < lines.len() {
        let line = &lines[end];
        let logical = line.strip_suffix('\r').unwrap_or(line);
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
    if let Some(heading) = heading_line {
        let mut end = heading + 1;
        while end < lines.len() {
            let logical = lines[end].strip_suffix('\r').unwrap_or(&lines[end]);
            let trimmed = logical.trim_start();
            // A section ends at the next heading of level two or less:
            // any `#`/`##` heading at column zero ends it. Deeper
            // headings (`###`+) stay inside the section.
            if !logical.starts_with([' ', '\t']) && trimmed.starts_with('#') {
                let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
                if hashes <= 2 {
                    // Require a space after the hashes to count as a
                    // heading (mirrors the ATX parser).
                    if trimmed
                        .as_bytes()
                        .get(hashes)
                        .is_some_and(|b| b.is_ascii_whitespace())
                    {
                        break;
                    }
                }
            }
            end += 1;
        }
        (heading + 1, end)
    } else {
        // Pre-H2 region: everything before the first H2 (or EOF).
        // Frontmatter, the early H1, and preamble stay; notes append
        // after the last note block inside it.
        let mut end = lines.len();
        for (index, line) in lines.iter().enumerate() {
            let logical = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = logical.trim_start();
            if !logical.starts_with([' ', '\t'])
                && trimmed.starts_with("##")
                && trimmed
                    .as_bytes()
                    .get(2)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                end = index;
                break;
            }
        }
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
    while index < end {
        let logical = lines[index].strip_suffix('\r').unwrap_or(&lines[index]);
        if is_blank(logical) {
            index += 1;
            continue;
        }
        if is_top_level_bullet(logical) {
            let block_end = bullet_block_end(lines, index, 0);
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
    if let Some(block_id) = block_id.as_deref() {
        super::pomodoro_adjust::reject_duplicate_block_id(
            &contents, block_id, target,
        )?;
    }
    let resolution = resolve_notebook_section(
        &contents,
        section.as_deref(),
        exact_section,
        &route,
    )?;
    // New-section creation appends the heading at EOF first.
    let mut lines = split_lines_keep(&contents);
    // `split('\n')` on an empty file yields `[""]`; normalize to empty so
    // insertion does not emit a stray blank line.
    if lines.len() == 1 && lines[0].is_empty() {
        lines.clear();
    }
    let had_trailing_newline = contents.is_empty() || contents.ends_with('\n');
    // Drop the synthetic trailing element from the split so indexes line
    // up with real lines; it is restored on join.
    if had_trailing_newline
        && !lines.is_empty()
        && lines.last().is_some_and(|l| l.is_empty())
    {
        lines.pop();
    }
    let section_title = resolution.title.clone();
    let section_action = resolution.action;
    let mut heading_line = resolution.heading_line;
    if section_action == Some("create") {
        let title = resolution.create_title.clone().unwrap_or_default();
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
    for line in &block {
        for token in line.split_whitespace() {
            if let Some(id) = token.strip_prefix('^') {
                let clean = id
                    .trim_end_matches([',', '.', ';', ':', '!', '?', ')', ']']);
                if clean.is_empty() || !capture_language::is_block_id(clean) {
                    continue;
                }
                if live_ids.contains(clean)
                    || !seen_new.insert(clean.to_string())
                {
                    return Err(CaptureError::io(format!(
                        "block ID ^{clean} already exists in {}",
                        target.display()
                    )));
                }
            }
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
    let updated = join_lines(&staged, ending);
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
    let had_trailing_newline = contents.is_empty() || contents.ends_with('\n');
    if had_trailing_newline
        && !lines.is_empty()
        && lines.last().is_some_and(|l| l.is_empty())
    {
        lines.pop();
    }
    let parent_width = parent_indent.chars().count();
    // Child indent: parent indent plus the file's dominant unit.
    let unit = super::sub_bullet::child_indent_unit(planner, target)
        .unwrap_or_else(|_| "\t".to_string());
    let child_indent = format!("{parent_indent}{unit}");
    let _ = parent_width;
    let authored_children: Vec<String> =
        render_notebook_children(&parsed.sub_bullets, &unit)
            .into_iter()
            .map(|line| format!("{parent_indent}{line}"))
            .collect();
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
                block.push(format!("{child_indent}{}", line.trim_start()));
            }
        }
    }
    // Duplicate check for any new IDs (terminal IDs on sub-note bodies
    // count, even though the kind carries no block_id).
    let live_ids: std::collections::HashSet<String> =
        block_ids_outside_fences(&contents)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
    for line in &block {
        for token in line.split_whitespace() {
            if let Some(id) = token.strip_prefix('^') {
                let clean = id
                    .trim_end_matches([',', '.', ';', ':', '!', '?', ')', ']']);
                if clean.is_empty() || !capture_language::is_block_id(clean) {
                    continue;
                }
                if live_ids.contains(clean) {
                    return Err(CaptureError::io(format!(
                        "block ID ^{clean} already exists in {}",
                        target.display()
                    )));
                }
            }
        }
    }
    let insert_at = bullet_block_end(
        &lines,
        parent_line,
        indent_width(&lines[parent_line]),
    );
    let mut staged = lines.clone();
    for (offset, line) in block.iter().enumerate() {
        staged.insert(insert_at + offset, line.clone());
    }
    let updated = join_lines(&staged, ending);
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

fn notebook_item_result(
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
