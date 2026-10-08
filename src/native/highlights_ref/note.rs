//! Note pipeline, body, and tasks-section insertion.
use super::*;
use crate::native::env as bob_env;

pub(super) fn pipeline_metadata(
    config: &Config,
    pdf: &Path,
    source_pdf_sha256: &str,
    note: &ParsedNote,
    sidecar: Option<&SidecarInput>,
    rendered_highlights: Option<&RenderedHighlights>,
    refresh_synced_at: bool,
) -> Result<PipelineMetadata> {
    let highlights_sidecar = match sidecar {
        Some(sidecar) => {
            Some(MarkerValue::String(source_pdf_value(config, &sidecar.path)))
        }
        None => note.frontmatter_value(FIELD_HIGHLIGHTS_SIDECAR),
    };
    let highlights_count = match rendered_highlights {
        Some(rendered) => Some(MarkerValue::Number(rendered.count.to_string())),
        None => note.frontmatter_value(FIELD_HIGHLIGHTS_COUNT),
    };
    let highlights_synced_at = if rendered_highlights.is_some() {
        if refresh_synced_at {
            Some(MarkerValue::String(current_timestamp()))
        } else {
            note.frontmatter_value(FIELD_HIGHLIGHTS_SYNCED_AT)
                .or_else(|| Some(MarkerValue::String(current_timestamp())))
        }
    } else {
        note.frontmatter_value(FIELD_HIGHLIGHTS_SYNCED_AT)
    };
    let ref_type = pdf_path_metadata(config, pdf)?.ref_type;
    // Preview creation timestamp for new notes only. Existing notes keep
    // their authored `created` line via frontmatter preservation, so no
    // fresh value is generated here. The executor finalizes a fresh value
    // immediately before writing a new note so dry-run previews never
    // become persisted historical timestamps.
    let created = (!note.exists()).then(new_note_created_timestamp);
    let audio = discover_companion_audio(config, pdf)
        .map(|path| companion_audio_vault_path(config, pdf, &path));

    Ok(PipelineMetadata {
        source_pdf: source_pdf_value(config, pdf),
        source_pdf_sha256: source_pdf_sha256.to_string(),
        ref_type,
        highlights_sidecar,
        highlights_count,
        highlights_synced_at,
        created,
        audio,
    })
}

/// Format the note-writing invocation's clock as a `created` frontmatter
/// value (`YYYY-MM-DDTHH:mm:ss±ZZZZ`), honoring the `BOB_NOW` override.
pub(super) fn new_note_created_timestamp() -> String {
    super::super::capture_project_note::format_created_timestamp(
        bob_env::current_datetime(),
    )
}

pub(super) fn default_note_body(
    pdf: &Path,
    projection: &Projection,
    source_pdf: &str,
    rendered_highlights: Option<&RenderedHighlights>,
    audio: Option<&str>,
) -> String {
    let title = note_title(pdf, projection);
    let highlights = rendered_highlights
        .map(|rendered| rendered.content.as_str())
        .unwrap_or("");
    let mut body = String::new();
    body.push('\n');
    body.push_str("# ");
    body.push_str(&title);
    body.push_str("\n\n");
    // A note generated already closed carries its close date from the
    // start, so the library index can read it back from the tracker.
    let mark = projection_pdf_task_mark(projection);
    body.push_str(&stamp_close_date(
        &format!(
            "- [{mark}] {} {} [[{source_pdf}]] {} {}",
            PDF_TASK_TAG,
            PDF_TASK_KIND_TAG,
            PDF_TASK_HIDE_TAG,
            PDF_TASK_BLOCK_ID
        ),
        mark,
    ));
    body.push_str("\n\n");
    if let Some(audio) = audio {
        body.push_str(&audio_embed_line(audio));
        body.push_str("\n\n");
    }
    body.push_str("## Highlights\n\n");
    body.push_str(MANAGED_BODY_BEGIN);
    body.push_str("\n\n");
    body.push_str(highlights);
    if !highlights.is_empty() && !highlights.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(MANAGED_BODY_END);
    body.push('\n');
    body
}

pub(super) fn note_type_frontmatter_line() -> String {
    format!(
        "{FIELD_NOTE_TYPE}: {}",
        MarkerValue::String(NOTE_TYPE_VALUE.to_string()).as_frontmatter_value()
    )
}

pub(super) fn push_command_managed_frontmatter_lines(
    lines: &mut Vec<String>,
    metadata: &PipelineMetadata,
) {
    lines.push(note_type_frontmatter_line());
    if let Some(ref_type) = &metadata.ref_type {
        lines.push(format!(
            "{FIELD_REF_TYPE}: {}",
            MarkerValue::String(ref_type.clone()).as_frontmatter_value()
        ));
    }
    if let Some(audio) = &metadata.audio {
        lines.push(format!(
            "{FIELD_AUDIO}: {}",
            MarkerValue::String(audio_frontmatter_value(audio))
                .as_frontmatter_value()
        ));
    }
}

pub(super) fn note_title(pdf: &Path, projection: &Projection) -> String {
    projection
        .get("title")
        .and_then(MarkerValue::as_string)
        .filter(|title| !title.trim().is_empty())
        .map(|title| title.trim().to_string())
        .or_else(|| {
            pdf.file_stem()
                .and_then(OsStr::to_str)
                .map(|stem| stem.replace(['_', '-'], " "))
        })
        .unwrap_or_else(|| "Reference".to_string())
}

pub(super) fn managed_region(body: &str) -> Result<Option<&str>> {
    let Some(begin) = body.find(MANAGED_BODY_BEGIN) else {
        return Ok(None);
    };
    let after_begin = begin + MANAGED_BODY_BEGIN.len();
    if body[after_begin..].contains(MANAGED_BODY_BEGIN) {
        return Err(CommandError::new(
            "reference note has multiple managed Highlights begin markers",
        ));
    }
    let Some(relative_end) = body[after_begin..].find(MANAGED_BODY_END) else {
        return Err(CommandError::new(
            "reference note has a managed Highlights begin marker without a matching end marker",
        ));
    };
    let end = after_begin + relative_end;
    if body[end + MANAGED_BODY_END.len()..].contains(MANAGED_BODY_END) {
        return Err(CommandError::new(
            "reference note has multiple managed Highlights end markers",
        ));
    }
    Ok(Some(body[after_begin..end].trim_matches('\n')))
}

pub(super) fn replace_managed_region(
    body: &str,
    replacement: &str,
) -> Result<String> {
    let begin = body.find(MANAGED_BODY_BEGIN).ok_or_else(|| {
        CommandError::new(
            "reference note is missing the managed Highlights begin marker",
        )
    })?;
    let after_begin = begin + MANAGED_BODY_BEGIN.len();
    let relative_end =
        body[after_begin..].find(MANAGED_BODY_END).ok_or_else(|| {
            CommandError::new(
                "reference note is missing the managed Highlights end marker",
            )
        })?;
    let end = after_begin + relative_end;
    let mut rendered = String::new();
    rendered.push_str(&body[..after_begin]);
    rendered.push_str("\n\n");
    rendered.push_str(replacement);
    if !replacement.is_empty() && !replacement.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str(&body[end..]);
    Ok(rendered)
}

pub(super) fn rewrite_pdf_task_checkbox_for_projection(
    body: &str,
    projection: &Projection,
) -> Result<String> {
    let mark = projection_pdf_task_mark(projection);
    if matches!(mark, ' ' | '*' | '/')
        && matches!(
            parse_pdf_task_line(body)?,
            PdfTaskLineState::Present(task) if task.mark == '?'
        )
    {
        return Ok(body.to_string());
    }
    replace_pdf_task_checkbox_mark(body, mark)
}

#[cfg(test)]
pub(super) fn insert_missing_annotation_tasks(
    config: &Config,
    body: &str,
    candidates: &[AnnotationTaskCandidate],
    created_date: &str,
) -> Result<String> {
    if candidates.is_empty() {
        return Ok(body.to_string());
    }

    let mut existing = existing_annotation_task_identities(body);
    let mut missing = Vec::new();
    for candidate in candidates {
        if existing.insert(candidate.identity.clone()) {
            missing.push(render_annotation_task_line(
                config,
                candidate,
                created_date,
            ));
        }
    }

    if missing.is_empty() {
        return Ok(body.to_string());
    }

    insert_annotation_task_lines_into_tasks_section(body, &missing)
}

/// Line index of the note's `Tasks` heading, ignoring fenced code and the
/// managed Highlights region.
pub(super) fn tasks_heading_line_index(body: &str) -> Option<usize> {
    let mut managed = false;
    let mut open_fence = None;
    for (index, line) in body.lines().enumerate() {
        let skip_managed = if managed {
            if line.contains(MANAGED_BODY_END) {
                managed = false;
            }
            true
        } else if line.contains(MANAGED_BODY_BEGIN) {
            managed = !line.contains(MANAGED_BODY_END);
            true
        } else {
            false
        };

        let skip_fence = if let Some(marker) = open_fence {
            if markdown::closes_fence(line, marker) {
                open_fence = None;
            }
            true
        } else if let Some(marker) = markdown::fence_marker(line) {
            open_fence = Some(marker);
            true
        } else {
            false
        };

        if skip_managed || skip_fence {
            continue;
        }

        if let Some((_, title)) = markdown::atx_heading(line)
            && title == TASKS_SECTION_TITLE
        {
            return Some(index);
        }
    }
    None
}

/// Exclusive end line index of the `Tasks` section that starts at
/// `heading_line_index`.
pub(super) fn tasks_section_end_line_index(
    body: &str,
    heading_line_index: usize,
) -> usize {
    let lines: Vec<&str> = body.lines().collect();
    let mut open_fence = None;
    for (index, line) in lines.iter().enumerate().skip(heading_line_index + 1) {
        if let Some(marker) = open_fence {
            if markdown::closes_fence(line, marker) {
                open_fence = None;
            }
            continue;
        }
        if let Some(marker) = markdown::fence_marker(line) {
            open_fence = Some(marker);
            continue;
        }
        if markdown::atx_heading(line).is_some()
            || line.contains(MANAGED_BODY_BEGIN)
        {
            return index;
        }
    }
    lines.len()
}

/// Ensure the body has a `Tasks` section, creating `## Tasks` one blank line
/// below the generated `^ref` task block (the task line plus its indented
/// child lines, e.g. a `DEPENDS ON` line). Returns the body and the heading's
/// line index.
pub(super) fn ensure_tasks_section(
    body: &str,
    pdf_task_line_index: usize,
) -> (String, usize) {
    if let Some(index) = tasks_heading_line_index(body) {
        return (body.to_string(), index);
    }
    let lines: Vec<&str> = body.lines().collect();
    let anchor =
        task_block_end_line_index(&lines, pdf_task_line_index, lines.len());
    let body = insert_lines_after(
        body,
        anchor,
        &[String::new(), TASKS_SECTION_HEADING.to_string()],
    );
    (body, anchor + 2)
}

pub(super) fn insert_lines_into_tasks_section(
    body: &str,
    heading_line_index: usize,
    lines: &[String],
) -> String {
    let body_lines: Vec<&str> = body.lines().collect();
    let section_end = tasks_section_end_line_index(body, heading_line_index);
    let (anchor, leading_blank) = match last_task_block_line_index(
        &body_lines,
        heading_line_index,
        section_end,
    ) {
        Some(index) => (index, false),
        None if body_lines.get(heading_line_index + 1).is_some_and(
            |line| heading_line_index + 1 < section_end && is_blank_line(line),
        ) =>
        {
            (heading_line_index + 1, false)
        }
        None => (heading_line_index, true),
    };

    let mut payload = Vec::with_capacity(lines.len() + 2);
    if leading_blank {
        payload.push(String::new());
    }
    payload.extend(lines.iter().cloned());
    if body_lines
        .get(anchor + 1)
        .is_some_and(|line| !is_blank_line(line))
    {
        payload.push(String::new());
    }
    insert_lines_after(body, anchor, &payload)
}

pub(super) fn last_task_block_line_index(
    lines: &[&str],
    heading_line_index: usize,
    section_end: usize,
) -> Option<usize> {
    let fenced =
        markdown::fenced_lines(lines, heading_line_index + 1..section_end);
    let mut last_task = None;
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(section_end)
        .skip(heading_line_index + 1)
    {
        if fenced.contains(&index) {
            continue;
        }
        if is_top_level_task_line(line) {
            last_task = Some(index);
        }
    }
    last_task.map(|task_index| {
        task_block_end_line_index(lines, task_index, section_end)
    })
}

pub(super) fn task_block_end_line_index(
    lines: &[&str],
    task_index: usize,
    section_end: usize,
) -> usize {
    let end = section_end.min(lines.len());
    let mut index = task_index + 1;
    while index < end {
        let line = lines[index];
        if is_indented_line(line)
            || (is_blank_line(line)
                && next_nonblank_is_indented(lines, index + 1, end))
        {
            index += 1;
            continue;
        }
        break;
    }
    index.saturating_sub(1)
}

pub(super) fn next_nonblank_is_indented(
    lines: &[&str],
    start_index: usize,
    end: usize,
) -> bool {
    lines[start_index..end.min(lines.len())]
        .iter()
        .copied()
        .find(|line| !is_blank_line(line))
        .is_some_and(is_indented_line)
}

pub(super) fn is_top_level_task_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- [") else {
        return false;
    };
    let mut chars = rest.chars();
    if chars.next().is_none() || chars.next() != Some(']') {
        return false;
    }
    let after_checkbox = chars.as_str();
    after_checkbox
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
        && after_checkbox.contains("#task")
}

pub(super) fn is_indented_line(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

pub(super) fn is_blank_line(line: &str) -> bool {
    line.trim().is_empty()
}

pub(super) fn insert_annotation_task_lines_into_tasks_section(
    body: &str,
    lines: &[String],
) -> Result<String> {
    if lines.is_empty() {
        return Ok(body.to_string());
    }

    let task_line = match parse_pdf_task_line(body)? {
        PdfTaskLineState::Present(task_line) => task_line,
        PdfTaskLineState::Missing => {
            return Err(CommandError::new(
                "reference note is missing the generated PDF task line with ^ref; cannot create annotation tasks",
            ));
        }
    };
    let (body, heading_line_index) =
        ensure_tasks_section(body, task_line.line_index);
    Ok(insert_lines_into_tasks_section(
        &body,
        heading_line_index,
        lines,
    ))
}

pub(super) fn append_task_lines(contents: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return contents.to_string();
    }

    let line_ending = preferred_line_ending(contents);
    let mut rendered = String::with_capacity(
        contents.len()
            + lines
                .iter()
                .map(|line| line.len() + line_ending.len())
                .sum::<usize>()
            + line_ending.len(),
    );
    rendered.push_str(contents);
    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push_str(line_ending);
    }
    for line in lines {
        rendered.push_str(line);
        rendered.push_str(line_ending);
    }
    rendered
}

pub(super) fn preferred_line_ending(contents: &str) -> &'static str {
    contents
        .find('\n')
        .map(|index| {
            if index > 0 && contents.as_bytes().get(index - 1) == Some(&b'\r') {
                "\r\n"
            } else {
                "\n"
            }
        })
        .unwrap_or("\n")
}

pub(super) fn insert_lines_after(
    body: &str,
    line_index: usize,
    lines: &[String],
) -> String {
    let mut rendered = String::with_capacity(
        body.len() + lines.iter().map(|line| line.len() + 1).sum::<usize>(),
    );
    for (index, segment) in body.split_inclusive('\n').enumerate() {
        rendered.push_str(segment);
        if index != line_index {
            continue;
        }

        let (_, line_ending) = split_line_segment(segment);
        let line_ending = if line_ending.is_empty() {
            rendered.push('\n');
            "\n"
        } else {
            line_ending
        };
        for line in lines {
            rendered.push_str(line);
            rendered.push_str(line_ending);
        }
    }
    rendered
}

pub(super) fn replace_pdf_task_checkbox_mark(
    body: &str,
    mark: char,
) -> Result<String> {
    let task_line = match parse_pdf_task_line(body)? {
        PdfTaskLineState::Missing => return Ok(body.to_string()),
        PdfTaskLineState::Present(task_line) => task_line,
    };

    if task_line.mark == mark {
        return Ok(body.to_string());
    }

    let mut rendered = String::with_capacity(body.len());
    for (line_index, segment) in body.split_inclusive('\n').enumerate() {
        if line_index != task_line.line_index {
            rendered.push_str(segment);
            continue;
        }
        let (line, line_ending) = split_line_segment(segment);
        let mut updated = String::with_capacity(line.len() + 32);
        updated.push_str(&line[..task_line.checkbox_mark_index]);
        updated.push(mark);
        updated.push_str(&line[task_line.checkbox_mark_index + 1..]);
        rendered.push_str(&stamp_close_date(&updated, mark));
        rendered.push_str(line_ending);
    }
    if !body.ends_with('\n') {
        // split_inclusive includes the final unterminated segment, so this
        // branch is only here to make the invariant obvious to future edits.
        debug_assert_eq!(
            rendered.lines().count(),
            body.lines().count(),
            "unterminated final line should be rewritten in the loop"
        );
    }
    Ok(rendered)
}

/// Stamp a completion or cancellation date when sync itself closes the
/// generated `^ref` task: a mark of `x` inserts ` [completion:: YYYY-MM-DD]`
/// immediately before the `^ref` token, and `-` inserts `[cancelled:: ...]`.
/// The date comes from `env::current_datetime()` (honoring `BOB_NOW`).
/// Existing fields are never touched — in either bracket or emoji form —
/// and a reopen never removes a date, so stamping is idempotent.
pub(super) fn stamp_close_date(line: &str, mark: char) -> String {
    let field = match mark {
        'x' | 'X' => "completion",
        '-' => "cancelled",
        _ => return line.to_string(),
    };
    let already_stamped = if field == "completion" {
        line.contains("[completion::") || line.contains('✅')
    } else {
        line.contains("[cancelled::") || line.contains('❌')
    };
    if already_stamped {
        return line.to_string();
    }
    let Some(token_start) = close_date_insert_position(line) else {
        return line.to_string();
    };
    let date = bob_env::current_datetime().date().format("%Y-%m-%d");
    format!(
        "{}[{field}:: {date}] {}",
        &line[..token_start],
        &line[token_start..]
    )
}

/// Byte position of the `^ref` token start when it is preceded by
/// whitespace, so the close-date field lands immediately before it.
pub(super) fn close_date_insert_position(line: &str) -> Option<usize> {
    let token_start = line.rfind(PDF_TASK_BLOCK_ID)?;
    if token_start == 0
        || !line.as_bytes()[token_start - 1].is_ascii_whitespace()
    {
        return None;
    }
    Some(token_start)
}

pub(super) fn split_line_segment(segment: &str) -> (&str, &str) {
    if let Some(line) = segment.strip_suffix("\r\n") {
        (line, "\r\n")
    } else if let Some(line) = segment.strip_suffix('\n') {
        (line, "\n")
    } else {
        (segment, "")
    }
}

pub(super) fn generated_block_ids(region: &str) -> BTreeSet<String> {
    region
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix('^')
                .filter(|id| id.starts_with("h-") && is_valid_block_id(id))
                .map(str::to_string)
        })
        .collect()
}

pub(super) fn is_valid_block_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-'
        })
}

pub(super) fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(super) fn current_local_date() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

pub(crate) fn configured_path(
    matches: &ArgMatches,
    arg_name: &str,
    env_name: &str,
    default_value: &str,
    bob_dir: &Path,
) -> PathBuf {
    let configured = matches
        .get_one::<OsString>(arg_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            bob_env::var_os(env_name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from(default_value));

    resolve_under_bob(bob_dir, &configured)
}

pub(super) fn resolve_under_bob(bob_dir: &Path, path: &Path) -> PathBuf {
    let expanded = bob_env::expand_tilde(path);
    if expanded.is_absolute() {
        expanded
    } else {
        bob_dir.join(expanded)
    }
}

pub(super) fn required_path(matches: &ArgMatches, name: &str) -> PathBuf {
    let value = matches
        .get_one::<OsString>(name)
        .expect("required argument is enforced by clap");
    bob_env::expand_tilde(&PathBuf::from(value))
}

pub(super) fn prefer_from_matches(matches: &ArgMatches) -> Option<Prefer> {
    matches
        .get_one::<String>("prefer")
        .map(|value| match value.as_str() {
            "marker" => Prefer::Marker,
            "frontmatter" => Prefer::Frontmatter,
            _ => unreachable!("clap value parser restricts prefer"),
        })
}
