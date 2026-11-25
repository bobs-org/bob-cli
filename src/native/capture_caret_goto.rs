//! Read-only `@route^id` caret Go To resolver.
//!
//! Lands on the unique line whose trailing block ID is exactly `^<block-id>`.
//! Never writes. Identifiers use `caret_goto` / `CaretGoto`; never "jump".

use std::{fs, path::Path};

use serde::Serialize;

use super::{
    capture, capture_block_ids, capture_dependency_tasks, capture_targets,
    capture_tasks, collect_done, markdown, note_tasks,
    projects::{
        frontmatter_is_area, frontmatter_is_notebook, frontmatter_is_project,
        frontmatter_value, is_markdown_file, parse_frontmatter, ProjectStatus,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaretGotoStatus {
    Found,
    MissingNote,
    MissingBlockId,
    DuplicateBlockId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GotoTargetKind {
    Task,
    NotebookNote,
    Bullet,
    Heading,
    Paragraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GotoSuggestionReason {
    Case,
    Prefix,
    Near,
    OtherNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GotoRow {
    pub(crate) block_id: String,
    pub(crate) note_path: String,
    pub(crate) target: String,
    pub(crate) line: usize,
    #[serde(rename = "ref")]
    pub(crate) task_ref: String,
    pub(crate) target_kind: GotoTargetKind,
    pub(crate) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status_type: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) closed: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GotoSuggestion {
    #[serde(flatten)]
    pub(crate) row: GotoRow,
    pub(crate) draft: String,
    pub(crate) reason: GotoSuggestionReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CaretGoto {
    pub(crate) status: CaretGotoStatus,
    pub(crate) route: String,
    pub(crate) block_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) landing: Option<GotoRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hits: Option<Vec<GotoRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) suggestions: Option<Vec<GotoSuggestion>>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Scoped-browse payload for the additive `block_id.goto` completion
/// object (cli-browse): every uniquely identified block in the living
/// note in document order, then the paired archive's unique blocks
/// flagged `archived`. IDs that occur more than once are omitted; the
/// parse card explains them. The rows are unfiltered; the Mac filters
/// as it does `used`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CaretBrowse {
    pub(crate) eligible: bool,
    pub(crate) note_path: String,
    pub(crate) target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) archive_path: Option<String>,
    pub(crate) rows: Vec<GotoRow>,
}

pub(crate) fn browse(bob_dir: &Path, route: &str) -> CaretBrowse {
    let route = route.to_ascii_lowercase();
    let note_path = capture::route_label(&route);
    let living_abs = bob_dir.join(&note_path);
    let target = living_abs.display().to_string();
    let Ok(living_contents) = fs::read_to_string(&living_abs) else {
        return CaretBrowse {
            eligible: true,
            note_path,
            target,
            archive_path: None,
            rows: Vec::new(),
        };
    };
    let settings = note_tasks::read_settings(bob_dir);
    let living_scan = note_tasks::scan(&living_contents, &settings);
    let living_notebook = is_notebook_contents(&living_contents);
    let mut rows = unique_rows(
        &note_path,
        &living_abs,
        &living_contents,
        &living_scan,
        living_notebook,
        false,
    );
    let mut archive_path = None;
    if let Some(archive) =
        archive_for_living(bob_dir, &note_path, &living_contents)
    {
        let archive_abs = bob_dir.join(&archive);
        if archive_abs.is_file()
            && let Ok(archive_contents) = fs::read_to_string(&archive_abs)
        {
            archive_path = Some(archive.display().to_string());
            let archive_scan = note_tasks::scan(&archive_contents, &settings);
            let archive_notebook = is_notebook_contents(&archive_contents);
            rows.extend(unique_rows(
                &archive.to_string_lossy(),
                &archive_abs,
                &archive_contents,
                &archive_scan,
                archive_notebook,
                true,
            ));
        }
    }
    CaretBrowse {
        eligible: true,
        note_path,
        target,
        archive_path,
        rows,
    }
}

/// Every block ID with exactly one hit, as a row in document order.
fn unique_rows(
    note_path: &str,
    abs: &Path,
    contents: &str,
    scan: &note_tasks::NoteTaskScan,
    notebook: bool,
    archived: bool,
) -> Vec<GotoRow> {
    let lines: Vec<&str> = contents.lines().collect();
    let mut rows: Vec<GotoRow> = scan
        .all_block_ids()
        .filter(|id| scan.hits_for(id).len() == 1)
        .map(|id| {
            let hit = &scan.hits_for(id)[0];
            let line = lines.get(hit.line_index).copied().unwrap_or("");
            row_from_hit(
                note_path,
                abs,
                contents,
                scan,
                id,
                hit.line_index,
                line,
                hit.task_index,
                notebook,
                archived,
            )
        })
        .collect();
    rows.sort_by_key(|row| row.line);
    rows
}

pub(crate) fn resolve(
    bob_dir: &Path,
    route: &str,
    block_id: &str,
) -> CaretGoto {
    let route = route.to_ascii_lowercase();
    let note_path = capture::route_label(&route);
    let living_abs = bob_dir.join(&note_path);
    if !living_abs.is_file() {
        return CaretGoto {
            status: CaretGotoStatus::MissingNote,
            route,
            block_id: block_id.to_string(),
            note_path: Some(note_path),
            target: None,
            archived: false,
            landing: None,
            hits: None,
            suggestions: None,
        };
    }
    let settings = note_tasks::read_settings(bob_dir);
    let living_contents = fs::read_to_string(&living_abs).unwrap_or_default();
    let living_scan = note_tasks::scan(&living_contents, &settings);
    let living_notebook = is_notebook_contents(&living_contents);
    let living_hits = rows_for_id(
        &note_path,
        &living_abs,
        &living_contents,
        &living_scan,
        block_id,
        living_notebook,
        false,
    );
    match living_hits.len() {
        1 => {
            return found(
                route,
                block_id,
                &note_path,
                &living_abs,
                living_hits,
            );
        }
        n if n > 1 => {
            return CaretGoto {
                status: CaretGotoStatus::DuplicateBlockId,
                route,
                block_id: block_id.to_string(),
                note_path: Some(note_path),
                target: Some(living_abs.display().to_string()),
                archived: false,
                landing: None,
                hits: Some(living_hits),
                suggestions: None,
            };
        }
        _ => {}
    }

    if let Some(archive) =
        archive_for_living(bob_dir, &note_path, &living_contents)
    {
        let archive_abs = bob_dir.join(&archive);
        if archive_abs.is_file() {
            let archive_contents =
                fs::read_to_string(&archive_abs).unwrap_or_default();
            if contents_may_contain_id(&archive_contents, block_id) {
                let archive_scan =
                    note_tasks::scan(&archive_contents, &settings);
                let archive_notebook = is_notebook_contents(&archive_contents);
                let archive_hits = rows_for_id(
                    &archive.to_string_lossy(),
                    &archive_abs,
                    &archive_contents,
                    &archive_scan,
                    block_id,
                    archive_notebook,
                    true,
                );
                match archive_hits.len() {
                    1 => {
                        return found(
                            route,
                            block_id,
                            &archive.to_string_lossy(),
                            &archive_abs,
                            archive_hits,
                        );
                    }
                    n if n > 1 => {
                        return CaretGoto {
                            status: CaretGotoStatus::DuplicateBlockId,
                            route,
                            block_id: block_id.to_string(),
                            note_path: Some(archive.display().to_string()),
                            target: Some(archive_abs.display().to_string()),
                            archived: true,
                            landing: None,
                            hits: Some(archive_hits),
                            suggestions: None,
                        };
                    }
                    _ => {}
                }
            }
        }
    }

    let suggestions = suggestions_for(
        bob_dir,
        &route,
        &note_path,
        &living_abs,
        &living_contents,
        &living_scan,
        block_id,
        living_notebook,
        &settings,
    );
    CaretGoto {
        status: CaretGotoStatus::MissingBlockId,
        route,
        block_id: block_id.to_string(),
        note_path: Some(note_path),
        target: Some(living_abs.display().to_string()),
        archived: false,
        landing: None,
        hits: None,
        suggestions: (!suggestions.is_empty()).then_some(suggestions),
    }
}

fn found(
    route: String,
    block_id: &str,
    note_path: &str,
    abs: &Path,
    mut hits: Vec<GotoRow>,
) -> CaretGoto {
    let landing = hits.pop();
    let archived = landing.as_ref().is_some_and(|row| row.archived);
    CaretGoto {
        status: CaretGotoStatus::Found,
        route,
        block_id: block_id.to_string(),
        note_path: Some(note_path.to_string()),
        target: Some(abs.display().to_string()),
        archived,
        landing,
        hits: None,
        suggestions: None,
    }
}

fn archive_for_living(
    bob_dir: &Path,
    note_path: &str,
    contents: &str,
) -> Option<std::path::PathBuf> {
    if let Some(linked) = collect_done::done_tasks_archive_path(contents) {
        return Some(linked);
    }
    let canonical =
        collect_done::pair::canonical_archive_path(Path::new(note_path))?;
    bob_dir.join(&canonical).is_file().then_some(canonical)
}

fn is_notebook_contents(contents: &str) -> bool {
    if let Some(frontmatter) = parse_frontmatter(contents) {
        return frontmatter_is_notebook(&frontmatter);
    }
    contents.contains("[[notebook]]")
}

fn rows_for_id(
    note_path: &str,
    abs: &Path,
    contents: &str,
    scan: &note_tasks::NoteTaskScan,
    block_id: &str,
    notebook: bool,
    archived: bool,
) -> Vec<GotoRow> {
    let lines: Vec<&str> = contents.lines().collect();
    scan.hits_for(block_id)
        .iter()
        .filter_map(|hit| {
            let line = lines.get(hit.line_index).copied().unwrap_or("");
            Some(row_from_hit(
                note_path,
                abs,
                contents,
                scan,
                block_id,
                hit.line_index,
                line,
                hit.task_index,
                notebook,
                archived,
            ))
        })
        .collect()
}

fn row_from_hit(
    note_path: &str,
    abs: &Path,
    contents: &str,
    scan: &note_tasks::NoteTaskScan,
    block_id: &str,
    line_index: usize,
    line: &str,
    task_index: Option<usize>,
    notebook: bool,
    archived: bool,
) -> GotoRow {
    let line_no = line_index + 1;
    if let Some(index) = task_index
        && let Some(task) = scan.tasks().get(index)
    {
        let closed =
            capture_dependency_tasks::closed_on_for(task.status_type, line);
        return GotoRow {
            block_id: block_id.to_string(),
            note_path: note_path.to_string(),
            target: abs.display().to_string(),
            line: line_no,
            task_ref: task.task_ref(),
            target_kind: GotoTargetKind::Task,
            text: task.description.clone(),
            status_symbol: Some(task.status_symbol.to_string()),
            status_name: Some(task.status_name.clone()),
            status_type: Some(
                capture_tasks::status_type_label(task.status_type).to_string(),
            ),
            section: task.section.clone(),
            closed,
            archived,
        };
    }
    let digest = note_tasks::line_digest(line);
    let (target_kind, text) = classify_nontask(line, block_id, notebook);
    GotoRow {
        block_id: block_id.to_string(),
        note_path: note_path.to_string(),
        target: abs.display().to_string(),
        line: line_no,
        task_ref: format!("{line_no}:{digest}"),
        target_kind,
        text,
        status_symbol: None,
        status_name: None,
        status_type: None,
        section: section_before(contents, line_index),
        closed: None,
        archived,
    }
}

fn classify_nontask(
    line: &str,
    block_id: &str,
    notebook: bool,
) -> (GotoTargetKind, String) {
    if let Some((_, title)) = markdown::atx_heading(line) {
        return (
            GotoTargetKind::Heading,
            capture_block_ids::nontask_text(title, block_id),
        );
    }
    if is_list_item(line) {
        let text = capture_block_ids::nontask_text(line, block_id);
        let kind = if notebook {
            GotoTargetKind::NotebookNote
        } else {
            GotoTargetKind::Bullet
        };
        return (kind, text);
    }
    (
        GotoTargetKind::Paragraph,
        capture_block_ids::nontask_text(line, block_id),
    )
}

fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
    {
        return true;
    }
    let digit_len = trimmed
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_len == 0 {
        return false;
    }
    let after = &trimmed[digit_len..];
    let mut chars = after.chars();
    matches!(chars.next(), Some('.') | Some(')'))
        && chars.next().is_some_and(char::is_whitespace)
}

fn section_before(contents: &str, line_index: usize) -> Option<String> {
    let lines: Vec<&str> = contents.lines().collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = markdown::fenced_lines(&lines, scan_start..lines.len());
    let mut section = None;
    for (index, line) in lines.iter().enumerate().take(line_index) {
        if frontmatter_end.is_some_and(|end| index <= end)
            || fenced.contains(&index)
        {
            continue;
        }
        if let Some((_, title)) = markdown::atx_heading(line) {
            section = Some(title.to_string());
        }
    }
    section
}

fn suggestions_for(
    bob_dir: &Path,
    route: &str,
    note_path: &str,
    abs: &Path,
    contents: &str,
    scan: &note_tasks::NoteTaskScan,
    typed: &str,
    notebook: bool,
    settings: &note_tasks::NoteTaskSettings,
) -> Vec<GotoSuggestion> {
    let mut ranked: Vec<(u8, GotoSuggestion)> = Vec::new();
    for id in scan.all_block_ids() {
        if id == typed {
            continue;
        }
        let reason = if id.eq_ignore_ascii_case(typed) {
            GotoSuggestionReason::Case
        } else if id.starts_with(typed) {
            GotoSuggestionReason::Prefix
        } else if edit_distance(id, typed) <= 2 {
            GotoSuggestionReason::Near
        } else {
            continue;
        };
        let Some(hit) = scan.hits_for(id).first() else {
            continue;
        };
        let lines: Vec<&str> = contents.lines().collect();
        let line = lines.get(hit.line_index).copied().unwrap_or("");
        let row = row_from_hit(
            note_path,
            abs,
            contents,
            scan,
            id,
            hit.line_index,
            line,
            hit.task_index,
            notebook,
            false,
        );
        let rank = match reason {
            GotoSuggestionReason::Case => 0,
            GotoSuggestionReason::Prefix => 1,
            GotoSuggestionReason::Near => 2,
            GotoSuggestionReason::OtherNote => 3,
        };
        ranked.push((
            rank,
            GotoSuggestion {
                draft: format!("@{route}^{id}"),
                reason,
                row,
            },
        ));
    }
    ranked.sort_by_key(|(rank, suggestion)| {
        (*rank, suggestion.row.line, suggestion.row.block_id.clone())
    });
    if !ranked.is_empty() {
        ranked.truncate(3);
        return ranked
            .into_iter()
            .map(|(_, suggestion)| suggestion)
            .collect();
    }

    other_note_suggestions(bob_dir, route, typed, settings)
}

fn other_note_suggestions(
    bob_dir: &Path,
    current_route: &str,
    typed: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> Vec<GotoSuggestion> {
    // Walk the same root markdown `/` reads (inbox, areas, open projects)
    // without `scan_capture_targets`, whose album-registry discovery is far
    // too expensive for a live capture-parse miss.
    let Ok(entries) = fs::read_dir(bob_dir) else {
        return Vec::new();
    };
    let needle = format!("^{typed}");
    let mut ranked = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_file())
            || !is_markdown_file(&path)
        {
            continue;
        }
        let Some(route) = capture_targets::routable_route_for_root_file(&path)
        else {
            continue;
        };
        if route.eq_ignore_ascii_case(current_route) {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        if !contents.contains(&needle) {
            continue;
        }
        let Some(kind_rank) = catalog_kind_rank(&route, &contents) else {
            continue;
        };
        let scan = note_tasks::scan(&contents, settings);
        let hits = scan.hits_for(typed);
        let Some(hit) = hits.first() else {
            continue;
        };
        let notebook = is_notebook_contents(&contents);
        let lines: Vec<&str> = contents.lines().collect();
        let line = lines.get(hit.line_index).copied().unwrap_or("");
        let note_path = capture::route_label(&route);
        let row = row_from_hit(
            &note_path,
            &path,
            &contents,
            &scan,
            typed,
            hit.line_index,
            line,
            hit.task_index,
            notebook,
            false,
        );
        ranked.push((
            kind_rank,
            route.clone(),
            GotoSuggestion {
                draft: format!("@{route}^{typed}"),
                reason: GotoSuggestionReason::OtherNote,
                row,
            },
        ));
    }
    ranked.sort_by(
        |(left_rank, left_route, left), (right_rank, right_route, right)| {
            left_rank
                .cmp(right_rank)
                .then_with(|| left_route.cmp(right_route))
                .then_with(|| left.row.line.cmp(&right.row.line))
        },
    );
    ranked.truncate(3);
    ranked
        .into_iter()
        .map(|(_, _, suggestion)| suggestion)
        .collect()
}

fn catalog_kind_rank(route: &str, contents: &str) -> Option<u8> {
    if route == capture::inbox_route() {
        return Some(0);
    }
    let frontmatter = parse_frontmatter(contents)?;
    if frontmatter_is_notebook(&frontmatter) {
        return None;
    }
    if frontmatter_is_area(&frontmatter) {
        return Some(1);
    }
    if !frontmatter_is_project(&frontmatter) {
        return None;
    }
    let status =
        ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
    (!status.is_terminal()).then_some(2)
}

fn contents_may_contain_id(contents: &str, block_id: &str) -> bool {
    let mut needle = String::with_capacity(block_id.len() + 1);
    needle.push('^');
    needle.push_str(block_id);
    contents.contains(&needle)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > 2 {
        return 3;
    }
    let mut prev: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (i, left_ch) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_ch) in right.iter().enumerate() {
            let cost = usize::from(left_ch != right_ch);
            current[j + 1] =
                (prev[j + 1] + 1).min(current[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut current);
    }
    prev[right.len()]
}
