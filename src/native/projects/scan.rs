//! Directory scanning and project/task/wikilink parsing.
use super::*;

pub(super) fn scan_projects(bob_dir: &Path) -> ScanReport {
    let mut projects = Vec::new();
    let mut issues = Vec::new();
    scan_directory(bob_dir, bob_dir, &mut projects, &mut issues);
    projects.sort_by(|left, right| {
        left.status
            .sort_rank()
            .cmp(&right.status.sort_rank())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    ScanReport { projects, issues }
}

pub(super) fn scan_directory(
    root: &Path,
    directory: &Path,
    projects: &mut Vec<Project>,
    issues: &mut Vec<ScanIssue>,
) {
    let entries = match read_sorted_directory(directory) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(ScanIssue::path(
                relative_or_original(root, directory),
                format!("failed to read directory: {error}"),
            ));
            return;
        }
    };

    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                issues.push(ScanIssue::path(
                    relative_or_original(root, &path),
                    format!("failed to inspect path: {error}"),
                ));
                continue;
            }
        };

        if file_type.is_dir() {
            if is_excluded_directory(&path) {
                continue;
            }
            scan_directory(root, &path, projects, issues);
            continue;
        }

        if file_type.is_file() && is_markdown_file(&path) {
            scan_markdown_file(root, &path, projects, issues);
        }
    }
}

pub(super) fn read_sorted_directory(
    directory: &Path,
) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());
    Ok(entries)
}

pub(super) fn scan_markdown_file(
    root: &Path,
    path: &Path,
    projects: &mut Vec<Project>,
    issues: &mut Vec<ScanIssue>,
) {
    let relative_path = relative_or_original(root, path);
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            issues.push(ScanIssue::path(
                relative_path,
                format!("failed to read file: {error}"),
            ));
            return;
        }
    };

    let Some(project) = parse_project(&relative_path, &contents, issues) else {
        return;
    };
    projects.push(project);
}

pub(super) fn parse_project(
    relative_path: &Path,
    contents: &str,
    issues: &mut Vec<ScanIssue>,
) -> Option<Project> {
    let frontmatter = parse_frontmatter(contents)?;
    if !frontmatter_is_project(&frontmatter) {
        return None;
    }

    let status =
        ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
    let parent_target =
        frontmatter_value(&frontmatter, "parent").and_then(wikilink_target);
    let scheduled = parse_project_schedule(relative_path, &frontmatter, issues);
    let mut open_task_count = 0;
    let mut open_unhidden_count = 0;
    let mut dash_visible_count = 0;
    let mut task_lines = Vec::new();
    let mut prj_candidates = Vec::new();
    let lines = line_spans(contents);
    let mut fence = None;
    let today = bob_env::current_datetime().date();

    for (line_index, line_span) in lines.iter().enumerate() {
        if line_span.line_number <= frontmatter.body_start_line {
            continue;
        }
        let line = trim_cr(&contents[line_span.start..line_span.end]);
        if markdown_fence_line(line, &mut fence) {
            continue;
        }
        let has_prj_anchor = has_trailing_prj_anchor(line);
        if has_prj_anchor {
            prj_candidates.push(PrjCandidate {
                line_number: line_span.line_number,
                line_index,
                line,
            });
        }

        let Some(task) = parse_task_line(line) else {
            continue;
        };
        let scheduled_fields = inline_field_spans(task.text, "scheduled");
        let scheduled_date = (scheduled_fields.len() == 1)
            .then(|| {
                let field = scheduled_fields[0];
                parse_inline_schedule_date(
                    task.text[field.value_start..field.value_end].trim(),
                )
            })
            .flatten();
        task_lines.push(ProjectTaskLine {
            line_number: line_span.line_number,
            mark: task.mark,
            hide_tag_count: tag_spans(task.text, HIDE_TAG).len(),
            is_prj: is_valid_prj_task_line(line, task),
            is_open_task: contains_task_tag(task.text) && task.status.is_open(),
            scheduled_field_count: scheduled_fields.len(),
            scheduled_date,
        });
        if !contains_task_tag(task.text) || !task.status.is_open() {
            continue;
        }

        open_task_count += 1;
        if !has_prj_anchor && !contains_hide_tag(task.text) {
            open_unhidden_count += 1;
            let future_scheduled = scheduled_fields.iter().any(|field| {
                parse_inline_schedule_date(
                    task.text[field.value_start..field.value_end].trim(),
                )
                .is_some_and(|date| date > today)
            });
            if task.mark != '?' && !future_scheduled {
                dash_visible_count += 1;
            }
        }
    }

    let sub_block = if prj_candidates.len() == 1 {
        parse_prj_sub_block(contents, &lines, prj_candidates[0].line_index)
    } else {
        PrjSubBlock::default()
    };
    let prj_task =
        classify_prj_task(relative_path, &prj_candidates, sub_block, issues);

    Some(Project {
        relative_path: relative_path.to_path_buf(),
        name: project_name(relative_path),
        link_name: project_link_name(relative_path),
        link_stem: project_link_stem(relative_path),
        parent_target,
        scheduled,
        status,
        open_task_count,
        open_unhidden_count,
        dash_visible_count,
        task_lines,
        prj_task,
    })
}

pub(super) fn parse_project_schedule(
    relative_path: &Path,
    frontmatter: &Frontmatter<'_>,
    issues: &mut Vec<ScanIssue>,
) -> Option<ProjectSchedule> {
    let fields = frontmatter
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let rest = line.strip_prefix("scheduled")?;
            let value = rest.strip_prefix(':')?;
            Some((index + 2, value.trim()))
        })
        .collect::<Vec<_>>();

    let Some(&(line_number, raw_value)) = fields.first() else {
        return None;
    };
    if fields.len() > 1 {
        issues.push(ScanIssue::line(
            relative_path,
            fields[1].0,
            "multiple scheduled properties found; keep exactly one",
        ));
        return None;
    }

    let value = trim_yaml_scalar(raw_value);
    if !is_exact_date_shape(value) {
        issues.push(ScanIssue::line(
            relative_path,
            line_number,
            "scheduled must be a calendar date in YYYY-MM-DD format",
        ));
        return None;
    }

    let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") else {
        issues.push(ScanIssue::line(
            relative_path,
            line_number,
            format!("scheduled is not a valid calendar date: {value}"),
        ));
        return None;
    };

    Some(ProjectSchedule {
        raw: value.to_string(),
        date,
    })
}

pub(super) fn is_exact_date_shape(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

pub(crate) fn parse_frontmatter(contents: &str) -> Option<Frontmatter<'_>> {
    let mut lines = contents.lines();
    let first = lines.next()?;
    if trim_cr(first) != "---" {
        return None;
    }

    let mut frontmatter_lines = Vec::new();
    for (line_count, line) in (2..).zip(lines) {
        let line = trim_cr(line);
        if line == "---" {
            return Some(Frontmatter {
                lines: frontmatter_lines,
                body_start_line: line_count,
            });
        }
        frontmatter_lines.push(line);
    }

    None
}

pub(crate) fn frontmatter_is_project(frontmatter: &Frontmatter<'_>) -> bool {
    frontmatter_has_type(frontmatter, "[[project]]")
}

pub(crate) fn frontmatter_is_area(frontmatter: &Frontmatter<'_>) -> bool {
    frontmatter_has_type(frontmatter, "[[area]]")
}

pub(super) fn frontmatter_has_type(
    frontmatter: &Frontmatter<'_>,
    expected: &str,
) -> bool {
    // Scalar forms: quoted, single-quoted, and bare (`type: [[project]]`).
    if let Some(raw) = frontmatter_value(frontmatter, "type") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            // Flow-list form: `type: ["[[project]]"]`.
            if trimmed.starts_with('[') {
                return flow_list_contains(trimmed, expected);
            }
            return trim_yaml_scalar(trimmed) == expected;
        }
    }
    // Block-list form:
    // `type:` followed by `  - "[[project]]"` lines.
    block_list_contains(frontmatter, expected)
}

fn flow_list_contains(raw: &str, expected: &str) -> bool {
    let inner = raw.trim().strip_prefix('[').unwrap_or(raw);
    let inner = inner.strip_suffix(']').unwrap_or(inner);
    inner.split(',').any(|item| {
        let item = item.trim();
        if item.is_empty() {
            return false;
        }
        // Bare YAML wikilinks parse as a nested array (`[["project"]]`);
        // accept the inner scalar too.
        let unquoted = trim_yaml_scalar(item).trim();
        unquoted == expected
            || unquoted.trim_matches(['[', ']', '"', '\'']).trim() == "project"
                && expected == "[[project]]"
            || unquoted.trim_matches(['[', ']', '"', '\'']).trim() == "area"
                && expected == "[[area]]"
    })
}

fn block_list_contains(frontmatter: &Frontmatter<'_>, expected: &str) -> bool {
    let mut in_list = false;
    for line in &frontmatter.lines {
        if !in_list {
            let Some(rest) = line.strip_prefix("type") else {
                continue;
            };
            let Some(value) = rest.strip_prefix(':') else {
                continue;
            };
            if !value.trim().is_empty() {
                return false;
            }
            in_list = true;
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix('-'))
        else {
            break;
        };
        let item = trim_yaml_scalar(item.trim());
        if item == expected {
            return true;
        }
        // Nested `[["project"]]` scalar inside a block list.
        let inner = item.trim_matches(['[', ']', '"', '\'']).trim();
        if inner == "project" && expected == "[[project]]" {
            return true;
        }
        if inner == "area" && expected == "[[area]]" {
            return true;
        }
    }
    false
}

pub(crate) fn frontmatter_value<'a>(
    frontmatter: &'a Frontmatter<'a>,
    key: &str,
) -> Option<&'a str> {
    for line in &frontmatter.lines {
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        return Some(value.trim());
    }
    None
}

pub(crate) fn trim_yaml_scalar(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// External names for an area or project note (`project_name_aliases`).
///
/// Reads the line-based frontmatter form in flow (`["bob-cli"]`) or block
/// (`- bob-cli`) shape. Returns the accepted aliases in file order plus one
/// warning per ignored entry; a non-list value warns once and yields none.
pub(crate) fn project_name_aliases(
    frontmatter: &Frontmatter<'_>,
) -> (Vec<String>, Vec<String>) {
    let key_index = frontmatter.lines.iter().position(|line| {
        line.strip_prefix("project_name_aliases")
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some()
    });
    let Some(key_index) = key_index else {
        return (Vec::new(), Vec::new());
    };
    let raw = frontmatter.lines[key_index]
        .strip_prefix("project_name_aliases")
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return parse_alias_block_list(&frontmatter.lines[key_index + 1..]);
    }
    if raw.starts_with('[') {
        return parse_alias_flow_list(raw);
    }
    (
        Vec::new(),
        vec![
            "project_name_aliases is not a list; expected flow ([\"name\"]) or block (\"- name\") form"
                .to_string(),
        ],
    )
}

fn parse_alias_flow_list(raw: &str) -> (Vec<String>, Vec<String>) {
    let Some(inner) = raw
        .strip_prefix('[')
        .and_then(|rest| rest.rfind(']').map(|end| &rest[..end]))
    else {
        return (
            Vec::new(),
            vec![
                "project_name_aliases list is not closed with ']'; ignoring it"
                    .to_string(),
            ],
        );
    };
    let mut aliases = Vec::new();
    let mut warnings = Vec::new();
    if inner.trim().is_empty() {
        return (aliases, warnings);
    }
    for item in split_flow_items(inner) {
        match classify_alias_entry(item) {
            Ok(alias) => {
                if !aliases.iter().any(|seen| seen == &alias) {
                    aliases.push(alias);
                }
            }
            Err(warning) => warnings.push(warning),
        }
    }
    (aliases, warnings)
}

fn parse_alias_block_list(lines: &[&str]) -> (Vec<String>, Vec<String>) {
    let mut aliases = Vec::new();
    let mut warnings = Vec::new();
    let mut saw_item = false;
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix('-'))
        else {
            break;
        };
        saw_item = true;
        // A `-` with no text after it is an empty entry.
        let item = item.strip_prefix(' ').unwrap_or(item);
        match classify_alias_entry(item) {
            Ok(alias) => {
                if !aliases.iter().any(|seen| seen == &alias) {
                    aliases.push(alias);
                }
            }
            Err(warning) => warnings.push(warning),
        }
    }
    if !saw_item {
        warnings.push(
            "project_name_aliases has no list items; ignoring it".to_string(),
        );
    }
    (aliases, warnings)
}

/// Split flow-list items on commas that sit outside quotes.
fn split_flow_items(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut quote = None;
    for (index, byte) in inner.char_indices() {
        match quote {
            Some(active) if byte == active => quote = None,
            Some(_) => {}
            None => {
                if byte == '"' || byte == '\'' {
                    quote = Some(byte);
                } else if byte == ',' {
                    items.push(inner[start..index].trim());
                    start = index + 1;
                }
            }
        }
    }
    items.push(inner[start..].trim());
    items
}

/// Classify one raw alias entry: quoted and bare-word strings are kept,
/// empty and YAML non-string scalars are reported and dropped.
fn classify_alias_entry(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(
            "project_name_aliases entry is empty; ignoring it".to_string()
        );
    }
    let bytes = trimmed.as_bytes();
    if trimmed.len() >= 2
        && ((bytes[0] == b'"' && bytes[trimmed.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[trimmed.len() - 1] == b'\''))
    {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if inner.is_empty() {
            return Err(
                "project_name_aliases entry is empty; ignoring it".to_string()
            );
        }
        return Ok(inner.to_string());
    }
    if is_yaml_non_string_scalar(trimmed) {
        return Err(format!(
            "project_name_aliases entry '{trimmed}' is not a string; ignoring it"
        ));
    }
    // Strip a trailing YAML comment (`name # note`) before keeping it.
    let without_comment = split_yaml_comment(trimmed).trim().to_string();
    if without_comment.is_empty() {
        return Err(
            "project_name_aliases entry is empty; ignoring it".to_string()
        );
    }
    Ok(without_comment)
}

fn is_yaml_non_string_scalar(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "~" | "null" | "true" | "false" | "yes" | "no" | "on" | "off"
    ) || value.parse::<f64>().is_ok_and(|_| {
        value.bytes().all(|byte| {
            byte.is_ascii_digit()
                || matches!(byte, b'.' | b'+' | b'-' | b'e' | b'E' | b'_')
        }) && !value.is_empty()
    })
}

fn split_yaml_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#'
            && (index == 0 || matches!(bytes[index - 1], b' ' | b'\t'))
        {
            return value[..index].trim_end();
        }
        index += 1;
    }
    value
}

/// One typed area/project note for the per-note Ready cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypedNote {
    /// Vault-relative path with forward slashes.
    pub(crate) path: String,
    /// File stem without `.md`.
    pub(crate) stem: String,
    /// `area` or `project`.
    pub(crate) kind: String,
    pub(crate) status: crate::native::projects::ProjectStatus,
    /// Parent note stem, when `parent: "[[...]]"` parses.
    pub(crate) parent: Option<String>,
    /// Raw `ready_cap` frontmatter value, if present and non-empty.
    pub(crate) ready_cap_raw: Option<String>,
}

/// Walk the vault for typed area/project notes, reusing the project
/// directory exclusions. Returns path, stem, kind, status, parent
/// stem, and the raw `ready_cap` value.
pub(crate) fn walk_typed_notes(bob_dir: &Path) -> Vec<TypedNote> {
    let mut notes = Vec::new();
    walk_typed_directory(bob_dir, bob_dir, &mut notes);
    notes.sort_by(|a: &TypedNote, b: &TypedNote| a.path.cmp(&b.path));
    notes
}

fn walk_typed_directory(root: &Path, dir: &Path, notes: &mut Vec<TypedNote>) {
    let entries = match read_sorted_directory(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if is_excluded_directory(&path) {
                continue;
            }
            walk_typed_directory(root, &path, notes);
            continue;
        }
        if !file_type.is_file() || !is_markdown_file(&path) {
            continue;
        }
        let relative = relative_or_original(root, &path);
        let relative_slash = relative.to_string_lossy().replace('\\', "/");
        // Contract exclusions beyond directories: `done/` and daily
        // paths are never per-note entries; `dash.md` never is either.
        if relative_slash.starts_with("done/")
            || relative_slash == "done"
            || relative_slash == "dash.md"
        {
            continue;
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            continue;
        };
        let kind = if frontmatter_is_area(&frontmatter) {
            "area"
        } else if frontmatter_is_project(&frontmatter) {
            "project"
        } else {
            continue;
        };
        let status = crate::native::projects::ProjectStatus::parse(
            frontmatter_value(&frontmatter, "status"),
        );
        let parent =
            frontmatter_value(&frontmatter, "parent").and_then(wikilink_target);
        let ready_cap_raw = frontmatter_value(&frontmatter, "ready_cap")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let stem = relative
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_string();
        notes.push(TypedNote {
            path: relative_slash,
            stem,
            kind: kind.to_string(),
            status,
            parent,
            ready_cap_raw,
        });
    }
}

pub(super) fn wikilink_target(value: &str) -> Option<String> {
    wikilink_ref(value).map(|target| target.link_name)
}

pub(super) fn wikilink_ref(value: &str) -> Option<WikilinkRef> {
    let value = trim_yaml_scalar(value);
    let inner = value.strip_prefix("[[")?.strip_suffix("]]")?.trim();
    wikilink_ref_from_inner(inner)
}

pub(super) fn wikilink_ref_from_inner(inner: &str) -> Option<WikilinkRef> {
    let before_alias =
        inner.split_once('|').map_or(inner, |(target, _)| target);
    let before_heading = before_alias
        .split_once('#')
        .map_or(before_alias, |(target, _)| target);
    let stem = before_heading.rsplit('/').next()?.trim();
    if stem.is_empty() {
        return None;
    }
    Some(WikilinkRef {
        link_name: stem.to_ascii_lowercase(),
        stem: stem.to_string(),
    })
}

pub(super) fn wikilink_refs_in_line(line: &str) -> Vec<WikilinkRef> {
    wikilink_spans_in_line(line)
        .into_iter()
        .map(|span| span.link)
        .collect()
}

pub(super) fn wikilink_spans_in_line(line: &str) -> Vec<WikilinkSpan> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(open_relative) = line[offset..].find("[[") {
        let open = offset + open_relative;
        let Some(close_relative) = line[open + 2..].find("]]") else {
            break;
        };
        let close = open + 2 + close_relative;
        if let Some(target) = wikilink_ref_from_inner(&line[open + 2..close]) {
            spans.push(WikilinkSpan {
                link: target,
                start: open,
                end: close + 2,
            });
        }
        offset = close + 2;
    }
    spans
}

pub(super) fn parse_prj_sub_block(
    contents: &str,
    lines: &[LineSpan],
    prj_line_index: usize,
) -> PrjSubBlock {
    let prj_line = lines[prj_line_index];
    let prj_text = trim_cr(&contents[prj_line.start..prj_line.end]);
    let prj_indent = leading_whitespace(prj_text);
    let mut block = PrjSubBlock {
        prj_indent: prj_indent.to_string(),
        lines: Vec::new(),
    };

    for line in lines.iter().skip(prj_line_index + 1) {
        let line_text = trim_cr(&contents[line.start..line.end]);
        if line_text.trim().is_empty() {
            break;
        }
        let indentation = leading_whitespace(line_text);
        if indentation.len() <= prj_indent.len()
            || !indentation.starts_with(prj_indent)
        {
            break;
        }
        let trimmed_text = line_text.trim_start().to_string();
        let is_marker = list_item_content(line_text).is_some_and(|content| {
            content.starts_with(SUBPROJECTS_MARKER_PREFIX)
        });
        block.lines.push(PrjSubBlockLine {
            line_number: line.line_number,
            indentation: indentation.to_string(),
            trimmed_text,
            is_marker,
            links: wikilink_refs_in_line(line_text),
        });
    }

    block
}

pub(super) fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .find_map(|(index, character)| {
            (!character.is_whitespace()).then_some(index)
        })
        .unwrap_or(line.len());
    &line[..end]
}

pub(super) fn list_item_content(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let bullet = trimmed.chars().next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let after_bullet = &trimmed[bullet.len_utf8()..];
    if !after_bullet.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some(after_bullet.trim_start())
}

pub(super) fn classify_prj_task(
    relative_path: &Path,
    candidates: &[PrjCandidate<'_>],
    sub_block: PrjSubBlock,
    issues: &mut Vec<ScanIssue>,
) -> PrjTask {
    if candidates.is_empty() {
        return PrjTask::missing();
    }

    if candidates.len() > 1 {
        issues.push(ScanIssue::line(
            relative_path,
            candidates[1].line_number,
            "multiple ^prj tasks found; keep exactly one project completion task",
        ));
        return PrjTask::invalid(PrjTaskState::Multiple);
    }

    let candidate = &candidates[0];
    let Some(task) = parse_task_line(candidate.line) else {
        issues.push(malformed_prj_issue(relative_path, candidate.line_number));
        return PrjTask::invalid(PrjTaskState::Malformed);
    };
    if !contains_task_tag(task.text) {
        issues.push(malformed_prj_issue(relative_path, candidate.line_number));
        return PrjTask::invalid(PrjTaskState::Malformed);
    }

    let description = task_description(task.text);
    let placeholder = description == PLACEHOLDER_CRITERIA;
    PrjTask {
        state: match task.status {
            TaskStatus::Open => PrjTaskState::Open,
            TaskStatus::Done => PrjTaskState::Done,
            TaskStatus::Canceled => PrjTaskState::Canceled,
        },
        scheduled: inline_field_value(task.text, "scheduled"),
        hidden: contains_hide_tag(task.text),
        description,
        placeholder,
        sub_block,
    }
}

pub(super) fn is_valid_prj_task_line(
    line: &str,
    task: ParsedTaskLine<'_>,
) -> bool {
    has_trailing_prj_anchor(line) && contains_task_tag(task.text)
}

pub(super) fn malformed_prj_issue(
    relative_path: &Path,
    line_number: usize,
) -> ScanIssue {
    ScanIssue::line(
        relative_path,
        line_number,
        format!("malformed ^prj task; expected `{PROJECT_TASK_SHAPE}`"),
    )
}

pub(super) fn parse_task_line(line: &str) -> Option<ParsedTaskLine<'_>> {
    let mut trimmed = line.trim_start();
    while let Some(after_quote) = trimmed.strip_prefix('>') {
        trimmed = after_quote.trim_start_matches([' ', '\t']);
    }
    let marker_end = markdown_list_marker_end(trimmed)?;
    let after_marker = &trimmed[marker_end..];
    if !after_marker.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }

    let after_marker = after_marker.trim_start();
    let after_open_bracket = after_marker.strip_prefix('[')?;
    let mark = after_open_bracket.chars().next()?;
    let after_mark = &after_open_bracket[mark.len_utf8()..];
    let after_close_bracket = after_mark.strip_prefix(']')?;
    if !after_close_bracket.is_empty()
        && !after_close_bracket
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        return None;
    }

    Some(ParsedTaskLine {
        mark,
        status: TaskStatus::from_mark(mark),
        text: after_close_bracket.trim_start(),
    })
}

pub(super) fn is_propagated_schedule_mark(mark: char) -> bool {
    matches!(mark, ' ' | '*' | '/' | '?')
}

pub(super) fn markdown_list_marker_end(line: &str) -> Option<usize> {
    let first = line.chars().next()?;
    if matches!(first, '-' | '*' | '+') {
        return Some(first.len_utf8());
    }

    let digit_end = line
        .char_indices()
        .take_while(|(_, character)| character.is_ascii_digit())
        .map(|(index, character)| index + character.len_utf8())
        .last()?;
    matches!(line[digit_end..].chars().next(), Some('.' | ')'))
        .then_some(digit_end + 1)
}

pub(crate) fn is_markdown_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "md")
}

pub(super) fn is_excluded_directory(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        is_always_excluded_note_directory_name(name)
            || name.to_str() == Some("done")
    })
}

pub(super) fn trim_cr(value: &str) -> &str {
    value.strip_suffix('\r').unwrap_or(value)
}

pub(super) fn is_inline_field_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}
