use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::{
    super::{
        capture,
        collect_done::{
            split_line_ending, vault_relative_link_target, MovedBlockTargets,
        },
        markdown, note_tasks,
        note_tasks::NoteTaskSettings,
        task_children, task_complete,
        task_dependencies::inline_code_spans,
        task_locator::{self, LocatedTask},
        task_move::{
            transplant::{self, LineRange, RepairedFile},
            VaultSnapshot,
        },
        task_status_hooks::after_list_marker,
    },
    containers::{self, ResolvedDestination},
    types::{
        locator_text, MergeContext, MergeError, MergeRequest, PreparedMerge,
        PreparedPayload, PreparedSource, SourceDisposition,
    },
};

struct SourceWork {
    prepared: PreparedSource,
    located: LocatedTask,
    start_offset: usize,
    child_ranges: Vec<LineRange>,
    child_fragments: Vec<(String, String)>,
}

struct RawPayload {
    source_index: usize,
    source_path: PathBuf,
    contents: String,
}

/// Refuse a source row that no merge can retire: a recurring task or a
/// project's `^prj` row. The engine checks this before structure; capture's
/// read-only `<` picker and `merge_in_pending` share it so a row they call
/// mergeable is one execution accepts.
pub(crate) fn source_identity_refusal(
    source_line: &str,
    task: &note_tasks::NoteTask,
    block_id: &str,
) -> Option<MergeError> {
    if task_complete::is_recurring_task_line(source_line) {
        return Some(MergeError::new(
            "merge_source_recurring",
            format!("^{block_id} is recurring"),
        ));
    }
    if task.block_id.as_deref() == Some("prj") {
        return Some(MergeError::new(
            "merge_source_project",
            "the ^prj task cannot be merged",
        ));
    }
    None
}

/// Count the open tasks nested inside `source`'s block. `tasks` and `lines`
/// come from one scan of the source note, so callers that check every row of
/// a note scan it once.
pub(crate) fn open_subtask_count(
    tasks: &[note_tasks::NoteTask],
    lines: &[capture::LineSpan<'_>],
    source: &note_tasks::NoteTask,
) -> usize {
    tasks
        .iter()
        .filter(|task| task.line_index > source.line_index)
        .filter(|task| task.status_type.is_open())
        .filter(|task| {
            lines
                .get(task.line_index)
                .is_some_and(|line| line.end <= source.block_end)
        })
        .count()
}

/// The refusal for a source that still has open subtasks, if it has any.
pub(crate) fn open_subtasks_refusal(
    source: &note_tasks::NoteTask,
    open_nested: usize,
) -> Option<MergeError> {
    (open_nested > 0).then(|| {
        MergeError::new(
            "merge_source_open_subtasks",
            format!(
                "“{}” has {open_nested} open subtasks; move or close ^{} first",
                source.description,
                source.block_id.as_deref().unwrap_or("?")
            ),
        )
    })
}

/// Validate source identities and structural relationships for the complete
/// request, including sources that are already closed. This deliberately does
/// not resolve or create the destination container, so an unchanged retry can
/// remain harmless when the original container has since disappeared.
pub(crate) fn validate_merge_request(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    context: &MergeContext<'_>,
) -> Result<(), MergeError> {
    if request.sources.is_empty() {
        return Err(MergeError::new(
            "merge_no_sources",
            "select at least one source task",
        ));
    }

    let mut sources = Vec::with_capacity(request.sources.len());
    let mut identities = BTreeSet::new();
    for locator in &request.sources {
        let located = task_locator::resolve_task(
            &snapshot.notes,
            &locator_text(locator),
            context.task_settings,
        )
        .map_err(|error| MergeError::new(error.code, error.message))?;
        let identity = (
            located.path.clone(),
            located.task.block_id.clone().unwrap_or_default(),
        );
        if !identities.insert(identity) {
            return Err(MergeError::new(
                "merge_duplicate_source",
                format!(
                    "source ^{} was selected more than once",
                    locator.block_id
                ),
            ));
        }
        let lines =
            capture::line_spans(note_contents(snapshot, &located.path)?);
        let source_line = lines
            .get(located.task.line_index)
            .map(|line| line.text)
            .ok_or_else(|| {
                MergeError::new(
                    "task_not_found",
                    format!(
                        "no task `^{}` in `{}`",
                        locator.block_id,
                        located.path.display()
                    ),
                )
            })?;
        if let Some(refusal) = source_identity_refusal(
            source_line,
            &located.task,
            &locator.block_id,
        ) {
            return Err(refusal);
        }
        sources.push(SourceWork {
            prepared: PreparedSource {
                locator: locator.clone(),
                path: located.path.clone(),
                line: located.line,
                block_id: located.task.block_id.clone().unwrap_or_default(),
                title: String::new(),
                previous_status: located.task.status_symbol,
                disposition: SourceDisposition::from(&located),
                moved_lines: 0,
                moved_notes: 0,
            },
            start_offset: line_start_offset(&lines, located.task.line_index),
            located,
            child_ranges: Vec::new(),
            child_fragments: Vec::new(),
        });
    }

    let target_task = target_task(snapshot, &request.target, context)?;
    validate_target_and_source_ranges(
        snapshot,
        &sources,
        target_task.as_ref(),
    )?;
    for source in &sources {
        source_payload(
            note_contents(snapshot, &source.located.path)?,
            &source.located,
            context,
        )?;
    }
    Ok(())
}

/// Prepare destination payloads by copying each source's authored body.
/// Sources are not cut here: retirement later cancels them in place.
pub(crate) fn prepare_task_merge(
    snapshot: &VaultSnapshot,
    request: &MergeRequest,
    context: &MergeContext<'_>,
) -> Result<PreparedMerge, MergeError> {
    if request.sources.is_empty() {
        return Err(MergeError::new(
            "merge_no_sources",
            "select at least one source task",
        ));
    }

    let mut sources = Vec::with_capacity(request.sources.len());
    let mut identities = BTreeSet::new();
    for locator in &request.sources {
        let located = task_locator::resolve_task(
            &snapshot.notes,
            &locator_text(locator),
            context.task_settings,
        )
        .map_err(|error| MergeError::new(error.code, error.message))?;
        let identity = (located.path.clone(), locator.block_id.clone());
        if !identities.insert(identity) {
            return Err(MergeError::new(
                "merge_duplicate_source",
                format!(
                    "source ^{} was selected more than once",
                    locator.block_id
                ),
            ));
        }
        let source_lines =
            capture::line_spans(note_contents(snapshot, &located.path)?);
        let source_line = source_lines
            .get(located.task.line_index)
            .map(|line| line.text)
            .ok_or_else(|| {
                MergeError::new(
                    "task_not_found",
                    format!(
                        "no task `^{}` in `{}`",
                        locator.block_id,
                        located.path.display()
                    ),
                )
            })?;
        if let Some(refusal) = source_identity_refusal(
            source_line,
            &located.task,
            &locator.block_id,
        ) {
            return Err(refusal);
        }
        let disposition = SourceDisposition::from(&located);
        let prepared = PreparedSource {
            locator: locator.clone(),
            path: located.path.clone(),
            line: located.line,
            block_id: locator.block_id.clone(),
            title: note_tasks::clean_description(
                &located.task.description,
                &context.task_settings.global_filter,
                located.task.block_id.as_deref(),
            ),
            previous_status: located.task.status_symbol,
            disposition,
            moved_lines: 0,
            moved_notes: 0,
        };
        let start_offset =
            line_start_offset(&source_lines, located.task.line_index);
        sources.push(SourceWork {
            prepared,
            located,
            start_offset,
            child_ranges: Vec::new(),
            child_fragments: Vec::new(),
        });
    }

    if sources
        .iter()
        .all(|source| source.prepared.disposition == SourceDisposition::Closed)
    {
        return Ok(PreparedMerge {
            container: None,
            sources: sources
                .into_iter()
                .map(|source| source.prepared)
                .collect(),
            payloads: Vec::new(),
            postimages: BTreeMap::new(),
            renamed_block_ids: Vec::new(),
            repaired_files: Vec::new(),
            read_paths: snapshot.notes.keys().cloned().collect(),
            warnings: Vec::new(),
            insertion_dest: None,
            insertion_origins: Vec::new(),
        });
    }

    let initial_destination =
        containers::resolve_destination(snapshot, &request.target, context)?;
    let target_task = target_task(snapshot, &request.target, context)?;

    validate_target_and_source_ranges(
        snapshot,
        &sources,
        target_task.as_ref(),
    )?;

    // Resolve destination relationships before inspecting source children so a
    // target nested in a source gets its specific refusal rather than the
    // broader open-subtasks refusal.
    for source in &mut sources {
        if source.prepared.disposition == SourceDisposition::Closed {
            continue;
        }
        let (child_ranges, child_fragments, moved_notes) = source_payload(
            note_contents(snapshot, &source.located.path)?,
            &source.located,
            context,
        )?;
        source.prepared.moved_lines = child_ranges
            .iter()
            .map(|range| range.end.saturating_sub(range.start))
            .sum();
        source.prepared.moved_notes = moved_notes;
        source.child_ranges = child_ranges;
        source.child_fragments = child_fragments;
    }

    let destination = initial_destination;
    let note_index = context
        .note_index
        .with_additional(snapshot.notes.keys().cloned());
    let raw_payloads = build_payloads(&sources, &destination);
    let empty_targets = MovedBlockTargets::new();
    let empty_id_map = BTreeMap::new();
    let mut prepared_payloads = Vec::with_capacity(raw_payloads.len());
    let mut payload_link_count = 0;
    let mut payload_dependency_count = 0;
    for raw in &raw_payloads {
        let source = &sources[raw.source_index];
        let source_target =
            vault_relative_link_target(&source.prepared.path, "merge source")
                .map_err(|error| {
                MergeError::new("merge_invalid_path", error.to_string())
            })?;
        let stayed_source_ids = snapshot
            .notes
            .get(&source.prepared.path)
            .map(|contents| {
                transplant::live_block_ids(contents)
                    .into_iter()
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let sanitized =
            sanitize_copied_payload(&raw.contents, context.task_settings);
        let repaired = transplant::repair_moved_fragment(
            &sanitized,
            &source.prepared.path,
            &source_target,
            &note_index,
            &empty_targets,
            &stayed_source_ids,
            &empty_id_map,
        );
        payload_link_count += repaired.link_count;
        payload_dependency_count += repaired.dependency_count;
        prepared_payloads.push(PreparedPayload {
            source_index: raw.source_index,
            source_path: raw.source_path.clone(),
            contents: repaired.contents,
        });
    }

    let fragments: Vec<String> = prepared_payloads
        .iter()
        .map(|payload| payload.contents.clone())
        .collect();
    let (destination_postimage, landing_lines) =
        transplant::insert_fragments_at_byte(
            &destination.contents,
            destination.container.insertion_offset,
            &fragments,
            destination.container.empty_heading,
        )
        .ok_or_else(|| {
            MergeError::new(
                "merge_container_failed",
                "destination insertion offset is not a character boundary",
            )
        })?;
    // Capture exact insertion extents in destination-postimage coordinates
    // using the sanitized fragments. Each payload group's lines share its
    // source identity; coincident text stays distinct by range, never by
    // search.
    let mut insertion_origins = Vec::with_capacity(raw_payloads.len());
    for ((raw, landing), prepared) in raw_payloads
        .iter()
        .zip(landing_lines.iter())
        .zip(prepared_payloads.iter())
    {
        let Some(source) = sources.get(raw.source_index) else {
            continue;
        };
        if source.prepared.disposition == SourceDisposition::Closed {
            continue;
        }
        insertion_origins.push(super::types::PayloadInsertion {
            source_path: source.prepared.path.clone(),
            source_block_id: source.prepared.block_id.clone(),
            start_line: *landing,
            line_count: prepared.contents.lines().count().max(1),
        });
    }

    let mut repaired_files = Vec::new();
    if payload_link_count > 0 || payload_dependency_count > 0 {
        repaired_files.push(RepairedFile {
            path: destination.container.path.clone(),
            link_count: payload_link_count,
            dependency_token_count: payload_dependency_count,
        });
    }
    let mut postimages = BTreeMap::new();
    if snapshot
        .notes
        .get(&destination.container.path)
        .map(String::as_str)
        != Some(destination_postimage.as_str())
    {
        postimages
            .insert(destination.container.path.clone(), destination_postimage);
    }
    let mut warnings = Vec::new();
    if let Some(warning) = destination.warning {
        warnings.push(warning);
    }
    let insertion_dest = Some(destination.container.path.clone());
    let sources = sources.into_iter().map(|source| source.prepared).collect();

    Ok(PreparedMerge {
        container: Some(destination.container),
        sources,
        payloads: prepared_payloads,
        postimages,
        renamed_block_ids: Vec::new(),
        repaired_files,
        read_paths: snapshot.notes.keys().cloned().collect(),
        warnings,
        insertion_dest,
        insertion_origins,
    })
}

fn source_payload(
    contents: &str,
    source: &LocatedTask,
    context: &MergeContext<'_>,
) -> Result<(Vec<LineRange>, Vec<(String, String)>, usize), MergeError> {
    let lines = capture::line_spans(contents);
    let tasks = note_tasks::scan(contents, context.task_settings);
    if let Some(refusal) = open_subtasks_refusal(
        &source.task,
        open_subtask_count(tasks.tasks(), &lines, &source.task),
    ) {
        return Err(refusal);
    }

    let children = task_children::classify_direct_children(
        contents,
        &source.task,
        context.task_settings,
    );
    let end_line = lines
        .iter()
        .position(|line| line.end > source.task.block_end)
        .unwrap_or(lines.len());
    let source_unit = capture::first_child_indentation(
        &lines,
        source.task.line_index,
        source.task.block_end,
        &source.task.indentation,
    )
    .and_then(|indent| {
        indent
            .strip_prefix(&source.task.indentation)
            .map(str::to_string)
    })
    .filter(|indent| !indent.is_empty())
    .unwrap_or_else(|| {
        capture::dominant_indent_unit(&lines)
            .unwrap_or("\t")
            .to_string()
    });
    // Direct list roots define the boundaries of managed child blocks, but
    // authored bytes also include continuations before the first root and
    // paragraphs, tables, or fences with no wrapper bullet. Copy the whole
    // task body, skipping only direct managed roots and their nested bodies.
    let body_start = source.task.line_index + 1;
    let mut ranges = Vec::new();
    let mut cursor = body_start;
    for (offset, child) in children.iter().enumerate() {
        if !child.kind.is_managed() {
            continue;
        }
        let end = children
            .get(offset + 1)
            .map(|next| next.line_index)
            .unwrap_or(end_line);
        if cursor < child.line_index {
            ranges.push(LineRange {
                start: cursor,
                end: child.line_index,
            });
        }
        cursor = cursor.max(end);
    }
    if cursor < end_line {
        ranges.push(LineRange {
            start: cursor,
            end: end_line,
        });
    }

    let mut fragments = Vec::new();
    for range in &ranges {
        let fragment =
            transplant::cut_line_range(contents, range).ok_or_else(|| {
                MergeError::new(
                    "merge_source_structure",
                    format!(
                        "could not read child block of ^{}",
                        source.task.block_id.as_deref().unwrap_or("?")
                    ),
                )
            })?;
        fragments.push((fragment, source_unit.clone()));
    }
    let authored_roots = children
        .iter()
        .filter(|child| !child.kind.is_managed())
        .count();
    let moved_notes = if authored_roots > 0 {
        authored_roots
    } else if fragments
        .iter()
        .any(|(fragment, _)| !fragment.trim().is_empty())
    {
        1
    } else {
        0
    };
    Ok((ranges, fragments, moved_notes))
}

fn build_payloads(
    sources: &[SourceWork],
    destination: &ResolvedDestination,
) -> Vec<RawPayload> {
    let ending = transplant::document_line_ending(&destination.contents);
    let mut payloads = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        if source.prepared.disposition == SourceDisposition::Closed {
            continue;
        }
        let mut group = format!(
            "{}- {}{ending}",
            destination.title_indent, source.prepared.title
        );
        for (fragment, source_unit) in &source.child_fragments {
            let child_indent = first_fragment_indent(fragment)
                .unwrap_or_else(|| source_unit.clone());
            let rebased = transplant::rebase_fragment(
                fragment,
                &child_indent,
                &destination.child_indent,
                source_unit,
                &destination.child_unit,
                true,
            );
            if !group.ends_with('\n') {
                group.push_str(ending);
            }
            group.push_str(&rebased);
        }
        payloads.push(RawPayload {
            source_index,
            source_path: source.prepared.path.clone(),
            contents: group,
        });
    }
    payloads
}

fn first_fragment_indent(fragment: &str) -> Option<String> {
    fragment
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(capture::leading_whitespace)
        .map(str::to_string)
}

/// Strip identity from a copied payload, then leave fenced and inline-code
/// text untouched. Live `^id` anchors and `[id:: …]` fields stay on S.
/// Only live identity outside inline and fenced code is stripped; literal
/// `[id:: …]`, `^id`, wiki links, and Markdown links inside code survive.
/// A plain authored note containing `#task` is not a task row.
fn sanitize_copied_payload(
    fragment: &str,
    settings: &NoteTaskSettings,
) -> String {
    strip_copied_id_fields(&strip_copied_anchors(fragment), settings)
}

fn strip_copied_anchors(fragment: &str) -> String {
    let fenced = copied_fenced_lines(fragment);
    let occurrences = transplant::fragment_occurrences(fragment);
    let mut updated = fragment.to_string();
    for occurrence in occurrences.iter().rev() {
        let line_index = fragment[..occurrence.start].matches('\n').count();
        if fenced.contains(&line_index) {
            continue;
        }
        let line_start = fragment[..occurrence.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line_end = fragment[occurrence.end..]
            .find('\n')
            .map_or(fragment.len(), |rel| occurrence.end + rel);
        let line = &fragment[line_start..line_end];
        let relative = occurrence.start.saturating_sub(line_start);
        if inline_code_spans(split_line_ending(line).0)
            .iter()
            .any(|span| {
                relative < span.end && occurrence.end - line_start > span.start
            })
        {
            continue;
        }
        let caret = occurrence.start.saturating_sub(1);
        if updated.as_bytes().get(caret) != Some(&b'^') {
            continue;
        }
        let mut strip_start = caret;
        if strip_start > 0 {
            let previous = updated.as_bytes()[strip_start - 1];
            if previous == b' ' || previous == b'\t' {
                strip_start -= 1;
            }
        }
        updated.replace_range(strip_start..occurrence.end, "");
    }
    updated
}

fn strip_copied_id_fields(
    fragment: &str,
    settings: &NoteTaskSettings,
) -> String {
    let lines: Vec<&str> = fragment.split_inclusive('\n').collect();
    let fenced = copied_fenced_lines(fragment);
    let mut result = String::with_capacity(fragment.len());
    for (index, line) in lines.iter().enumerate() {
        let (content, ending) = split_line_ending(line);
        if fenced.contains(&index) || !is_copied_task_row(content, settings) {
            result.push_str(line);
            continue;
        }
        let cleaned = remove_live_id_fields(content);
        result.push_str(&cleaned);
        result.push_str(ending);
    }
    result
}

/// A merge-copy task row is a list item with a checkbox whose body carries
/// the live global filter (usually `#task`) outside inline code. Plain prose
/// that merely mentions `#task` is not a task row, and a `#task` that only
/// appears inside backticks does not make the line a task.
fn is_copied_task_row(content: &str, settings: &NoteTaskSettings) -> bool {
    let indent = content
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let Some(marker_end) = after_list_marker(content, indent) else {
        return false;
    };
    let mut index = marker_end;
    while content
        .as_bytes()
        .get(index)
        .is_some_and(u8::is_ascii_whitespace)
    {
        index += 1;
    }
    if content.as_bytes().get(index) != Some(&b'[') {
        return false;
    }
    let Some(close_relative) = content[index + 1..].find(']') else {
        return false;
    };
    let close = index + 1 + close_relative;
    let status_text = &content[index + 1..close];
    let mut status_chars = status_text.chars();
    let Some(_status) = status_chars.next() else {
        return false;
    };
    if status_chars.next().is_some() {
        return false;
    }
    let after_checkbox = &content[close + 1..];
    if !after_checkbox.is_empty()
        && !after_checkbox.starts_with(char::is_whitespace)
    {
        return false;
    }
    let body = after_checkbox.trim_start();
    if settings.global_filter.is_empty() {
        return true;
    }
    let code_spans = inline_code_spans(content);
    let mut offset = 0;
    for token in body.split_whitespace() {
        let Some(relative) = body[offset..].find(token) else {
            break;
        };
        let token_start_in_body = offset + relative;
        let token_start = content.len() - body.len() + token_start_in_body;
        let token_end = token_start + token.len();
        offset = token_start_in_body + token.len();
        if token != settings.global_filter {
            continue;
        }
        if code_spans
            .iter()
            .any(|span| token_start < span.end && token_end > span.start)
        {
            continue;
        }
        return true;
    }
    false
}

struct CopiedFieldSpan {
    start: usize,
    end: usize,
}

/// Live `[id:: …]` / `(id:: …)` spans outside inline code.
fn copied_live_field_spans(
    text: &str,
    key: &str,
    code_spans: &[std::ops::Range<usize>],
) -> Vec<CopiedFieldSpan> {
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let square = text[offset..].find('[');
        let parenthesis = text[offset..].find('(');
        let Some(open_relative) = (match (square, parenthesis) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        }) else {
            break;
        };
        let open = offset + open_relative;
        let opening = text.as_bytes()[open];
        let closing = if opening == b'[' { ']' } else { ')' };
        let Some(close_relative) = text[open + 1..].find(closing) else {
            offset = open + 1;
            continue;
        };
        let close = open + 1 + close_relative;
        let inner = &text[open + 1..close];
        if let Some((field_key, value)) = inner.split_once("::")
            && field_key.trim() == key
            && !code_spans
                .iter()
                .any(|span| open < span.end && close + 1 > span.start)
        {
            let value_start = open + 1 + field_key.len() + "::".len();
            let _ = value;
            fields.push(CopiedFieldSpan {
                start: open,
                end: close + 1,
                // value offsets retained for parity with the canonical
                // scanner; removal only needs the outer span.
                // value_start,
                // value_end: value_start + value.len(),
            });
            let _ = value_start;
        }
        offset = close + 1;
    }
    fields
}

fn remove_live_id_fields(line: &str) -> String {
    let code_spans = inline_code_spans(line);
    let spans = copied_live_field_spans(line, "id", &code_spans);
    let raw_spans = spans
        .iter()
        .map(|field| (field.start, field.end))
        .collect::<Vec<_>>();
    remove_copied_field_spans(line, 0, &raw_spans)
}

fn remove_copied_field_spans(
    line: &str,
    task_text_offset: usize,
    spans: &[(usize, usize)],
) -> String {
    let mut removal_ranges = spans
        .iter()
        .map(|(start, end)| {
            copied_field_removal_range(
                line,
                task_text_offset + start,
                task_text_offset + end,
            )
        })
        .collect::<Vec<_>>();
    removal_ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for range in removal_ranges {
        if let Some(last) = merged.last_mut()
            && range.0 <= last.1
        {
            last.1 = last.1.max(range.1);
        } else {
            merged.push(range);
        }
    }
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in merged {
        output.push_str(&line[cursor..start]);
        cursor = end;
    }
    output.push_str(&line[cursor..]);
    output
}

fn copied_field_removal_range(
    line_text: &str,
    field_start: usize,
    field_end: usize,
) -> (usize, usize) {
    let bytes = line_text.as_bytes();
    let mut after = field_end;
    while after < bytes.len() && is_copied_field_space(bytes[after]) {
        after += 1;
    }
    if after > field_end {
        return (field_start, after);
    }
    let mut before = field_start;
    while before > 0 && is_copied_field_space(bytes[before - 1]) {
        before -= 1;
    }
    (before, field_end)
}

fn is_copied_field_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn copied_fenced_lines(fragment: &str) -> BTreeSet<usize> {
    let lines: Vec<&str> = fragment.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let mut fenced = markdown::fenced_lines(&logical, 0..logical.len());
    let mut marker: Option<(char, usize)> = None;
    for (index, line) in logical.iter().enumerate() {
        let trimmed = line.trim_start_matches([' ', '\t']);
        if let Some(open) = marker {
            fenced.insert(index);
            if copied_fence_closes(trimmed, open) {
                marker = None;
            }
        } else if let Some(open) = copied_fence_marker(trimmed) {
            fenced.insert(index);
            marker = Some(open);
        }
    }
    fenced
}

fn copied_fence_marker(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = line
        .chars()
        .take_while(|character| *character == marker)
        .count();
    (count >= 3).then_some((marker, count))
}

fn copied_fence_closes(line: &str, open: (char, usize)) -> bool {
    let (marker, length) = open;
    let count = line
        .chars()
        .take_while(|character| *character == marker)
        .count();
    count >= length.max(3) && line[count..].trim().is_empty()
}

fn validate_target_and_source_ranges(
    snapshot: &VaultSnapshot,
    sources: &[SourceWork],
    target_task: Option<&LocatedTask>,
) -> Result<(), MergeError> {
    if let Some(target_task) = target_task {
        if sources.iter().any(|source| {
            source.prepared.path == target_task.path
                && source.prepared.block_id
                    == target_task.task.block_id.as_deref().unwrap_or_default()
        }) {
            return Err(MergeError::new(
                "merge_target_is_source",
                "the destination task is also a source",
            ));
        }
        if sources.iter().any(|source| {
            if source.prepared.path != target_task.path {
                return false;
            }
            let Some(contents) = snapshot.notes.get(&target_task.path) else {
                return false;
            };
            let lines = capture::line_spans(contents);
            let target_start =
                line_start_offset(&lines, target_task.task.line_index);
            target_start > source.start_offset
                && target_start < source.located.task.block_end
        }) {
            return Err(MergeError::new(
                "merge_target_inside_source",
                "the destination task is nested under a source task",
            ));
        }
    }

    for (index, left) in sources.iter().enumerate() {
        for right in &sources[index + 1..] {
            if left.prepared.path != right.prepared.path {
                continue;
            }
            let left_range = left.start_offset..left.located.task.block_end;
            let right_range = right.start_offset..right.located.task.block_end;
            if left_range.start < right_range.end
                && right_range.start < left_range.end
            {
                return Err(MergeError::new(
                    "merge_source_overlap",
                    format!(
                        "source task blocks ^{} and ^{} overlap",
                        left.prepared.block_id, right.prepared.block_id
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn target_task(
    snapshot: &VaultSnapshot,
    container: &super::types::MergeContainer,
    context: &MergeContext<'_>,
) -> Result<Option<LocatedTask>, MergeError> {
    match container {
        super::types::MergeContainer::Task(locator)
        | super::types::MergeContainer::Drawer(locator, _) => {
            containers::resolve_task(snapshot, locator, context).map(Some)
        }
        super::types::MergeContainer::Section { .. } => Ok(None),
    }
}

fn note_contents<'a>(
    snapshot: &'a VaultSnapshot,
    path: &Path,
) -> Result<&'a str, MergeError> {
    snapshot.notes.get(path).map(String::as_str).ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", path.display()),
        )
    })
}

fn line_start_offset(lines: &[capture::LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines.get(index - 1).map(|line| line.end).unwrap_or(0)
    }
}
