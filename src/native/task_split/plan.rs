//! Snapshot-only planner for `bob task split`.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;
use serde::Serialize;

use crate::native::{
    capture_block_ids::mint_block_id,
    collect_done::{block_ids_in_markdown, trailing_block_id_in_line},
    note_tasks::{self, NoteTaskSettings},
    task_children::{self, TaskChildKind},
    task_move::VaultSnapshot,
    task_relations,
};

static BLOCK_ID_BEFORE_INLINE_FIELDS_RE: LazyLock<Regex> =
    LazyLock::new(|| {
        Regex::new(
        r"\s+\^([A-Za-z0-9-]+)(?:\s+\[[A-Za-z][A-Za-z0-9_-]*::[^\]]*\])+\s*$",
    )
    .expect("valid block ID before inline fields regex")
    });

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum SplitSelection {
    Cursor { line: usize },
    Count { line: usize, count: usize },
    Lines { from: usize, to: usize },
    Nth(Vec<usize>),
    Match(Vec<String>),
    Section { title: String },
    BulletRef(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitRequest {
    pub(crate) owner_path: PathBuf,
    /// One-based task or heading line. Ignored for `Section` when title is set.
    pub(crate) owner_line: usize,
    pub(crate) selection: SplitSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitSettings {
    pub(crate) linked_from_lines: bool,
    pub(crate) linked_from_mentions: bool,
    pub(crate) inbox_route: String,
}

impl Default for SplitSettings {
    fn default() -> Self {
        Self {
            linked_from_lines: true,
            linked_from_mentions: true,
            inbox_route: "mac_inbox.md".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SplitTask {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: String,
    pub(crate) title: String,
    pub(crate) anchor_reused: bool,
    pub(crate) child_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SplitStub {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SplitPlacement {
    pub(crate) kind: &'static str,
    pub(crate) path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitPlan {
    pub(crate) postimages: BTreeMap<PathBuf, String>,
    pub(crate) owner_path: PathBuf,
    pub(crate) owner_line: usize,
    pub(crate) owner_block_id: Option<String>,
    pub(crate) owner_minted_id: Option<String>,
    pub(crate) tasks: Vec<SplitTask>,
    pub(crate) stubs: Vec<SplitStub>,
    pub(crate) placement: SplitPlacement,
    pub(crate) notice: String,
    pub(crate) cursor_path: String,
    pub(crate) cursor_line: usize,
    pub(crate) clamped: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SplitCandidate {
    pub(crate) ordinal: usize,
    pub(crate) line: usize,
    pub(crate) title: String,
    pub(crate) drawer: Option<String>,
    pub(crate) child_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    line: usize,
    end: usize,
    indent: String,
    raw_body: String,
    title: String,
    child_lines: Vec<String>,
    drawer_title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerKind {
    Task,
    Section,
}

#[derive(Debug, Clone)]
struct SelectedOwner {
    kind: OwnerKind,
    line: usize,
    end: usize,
    title: String,
    block_id: Option<String>,
}

pub(crate) fn plan_task_split(
    snapshot: &VaultSnapshot,
    request: &SplitRequest,
    today: &str,
    settings: &SplitSettings,
    task_settings: &NoteTaskSettings,
) -> Result<SplitPlan, SplitError> {
    let source = snapshot.notes.get(&request.owner_path).ok_or_else(|| {
        error(
            "split_owner_missing",
            format!(
                "{} does not exist in the vault",
                request.owner_path.display()
            ),
        )
    })?;
    let mut files = snapshot.notes.clone();
    let owner =
        resolve_owner(source, &request.owner_path, request, task_settings)?;
    let candidates = collect_candidates(
        source,
        owner.line,
        owner.end,
        owner.kind,
        task_settings,
    );
    let mut selected =
        select_candidates(&candidates, &request.selection, source, &owner)?;
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() {
        return Err(error(
            "split_no_selection",
            "no splittable bullets selected",
        ));
    }

    let placement = choose_placement(
        snapshot,
        &request.owner_path,
        source,
        &owner,
        &settings.inbox_route,
    );
    let destination_path =
        placement_path(snapshot, &request.owner_path, &placement)?;
    let destination_before =
        files.get(&destination_path).cloned().unwrap_or_default();
    let destination_is_source = destination_path == request.owner_path;
    let used_source: HashSet<String> =
        block_ids_in_markdown(source).into_iter().collect();
    let used_dest: HashSet<String> = block_ids_in_markdown(&destination_before)
        .into_iter()
        .collect();
    let mut used = if destination_is_source {
        used_source.clone()
    } else {
        used_dest.clone()
    };
    let owner_block_id = owner.block_id.clone();
    if let Some(id) = &owner_block_id {
        used.insert(id.clone());
    }
    let owner_minted_id = if owner.kind == OwnerKind::Task
        && owner.block_id.is_none()
        && settings.linked_from_lines
        && settings.linked_from_mentions
    {
        let id = mint_block_id(
            &owner.title,
            &used.iter().map(String::as_str).collect(),
        );
        used.insert(id.clone());
        Some(id)
    } else {
        None
    };
    let mention_id = owner_block_id.as_deref().or(owner_minted_id.as_deref());

    let source_lines = split_lines(source);
    let mut transformed_source = source_lines.clone();
    if let Some(id) = &owner_minted_id {
        transformed_source[owner.line] =
            set_trailing_block_id(&transformed_source[owner.line], id);
    }
    let mut generated = Vec::new();
    let mut planned_tasks = Vec::new();
    let mut stubs = Vec::new();
    for candidate_index in &selected {
        let candidate = &candidates[*candidate_index];
        let title = candidate.title.trim().to_string();
        let anchor = trailing_block_id_in_line(&candidate.raw_body);
        let id = if let Some(anchor) = anchor.as_ref() {
            used.insert(anchor.clone());
            anchor.clone()
        } else {
            let flat = flatten_for_id(&title);
            let id = mint_block_id(
                &flat,
                &used.iter().map(String::as_str).collect(),
            );
            used.insert(id.clone());
            id
        };
        let alias = alias_for(&title);
        let link = format!(
            "[[{}#^{id}|{alias}]]",
            if destination_path == request.owner_path {
                "".to_string()
            } else {
                note_target(&destination_path)
            }
        );
        let original_line = transformed_source[candidate.line].clone();
        let (_, ending) = split_ending(&original_line);
        let stub = format!("{}- {link}{ending}", candidate.indent);
        transformed_source[candidate.line] = stub.clone();
        for row in candidate.line + 1..=candidate.end {
            transformed_source[row].clear();
        }
        stubs.push(SplitStub {
            path: slash_path(&request.owner_path),
            line: candidate.line + 1,
            text: stub.trim().to_string(),
        });

        let task_line = format!(
            "{}- [ ] #task {} [created:: {}] ^{}\n",
            owner_indent(&source_lines[owner.line]),
            title,
            today,
            id
        );
        let base_indent = owner_indent(&source_lines[owner.line]);
        let child_indent = format!("{base_indent}  ");
        let mut children = rebase_children(
            &candidate.child_lines,
            &candidate.indent,
            &base_indent,
        );
        if settings.linked_from_lines
            && settings.linked_from_mentions
            && mention_id.is_some()
            && owner.kind == OwnerKind::Task
        {
            let parent_target = if destination_path == request.owner_path {
                format!("#^{}", mention_id.unwrap())
            } else {
                format!(
                    "{}#^{}",
                    note_target(&request.owner_path),
                    mention_id.unwrap()
                )
            };
            let mention = task_relations::format::format_linked_from_line(
                &child_indent,
                &[(
                    format!("[[{parent_target}]]"),
                    Some(task_relations::MENTION_LABEL.to_string()),
                )],
            );
            children.push(format!("{mention}\n"));
        }
        let child_count = candidate.child_lines.len();
        planned_tasks.push(SplitTask {
            path: slash_path(&destination_path),
            line: 0,
            block_id: id,
            title,
            anchor_reused: anchor.is_some(),
            child_lines: child_count,
        });
        generated.push(task_line);
        generated.extend(children);
    }

    let mut source_after = transformed_source.concat();
    if destination_is_source {
        if owner.kind == OwnerKind::Task {
            insert_after_block(&mut source_after, owner.end, &generated);
        } else {
            insert_tasks_section(&mut source_after, &generated)?;
        }
        files.insert(request.owner_path.clone(), source_after.clone());
    } else {
        files.insert(request.owner_path.clone(), source_after.clone());
        let dest_after = if owner.kind == OwnerKind::Task {
            insert_tasks_section_string(&destination_before, &generated)?
        } else {
            insert_tasks_section_string(&destination_before, &generated)?
        };
        files.insert(destination_path.clone(), dest_after);
    }

    // The parent id may have been minted after the source was snapshotted.
    if let Some(id) = &owner_minted_id {
        if let Some(contents) = files.get_mut(&request.owner_path) {
            let mut lines = split_lines(contents);
            if owner.line < lines.len() {
                lines[owner.line] =
                    set_trailing_block_id(&lines[owner.line], id);
                *contents = lines.concat();
            }
        }
    }

    for task in &mut planned_tasks {
        task.line = locate_line(&files[&destination_path], &task.block_id)
            .unwrap_or(0)
            + 1;
    }
    let first = planned_tasks.first().expect("selected is nonempty");
    let display_titles = planned_tasks
        .iter()
        .map(|task| format!("◯ {}", short_notice_title(&task.title)))
        .collect::<Vec<_>>()
        .join(" · ");
    let notice = format!(
        "✂ Split {} {} into tasks · {display_titles}",
        planned_tasks.len(),
        if planned_tasks.len() == 1 {
            "note"
        } else {
            "notes"
        }
    );
    let cursor_line = locate_line(&files[&destination_path], &first.block_id)
        .unwrap_or(0)
        + 1;
    let clamped = match request.selection {
        SplitSelection::Count { count, .. } if count > planned_tasks.len() => {
            Some((count, planned_tasks.len()))
        }
        _ => None,
    };
    Ok(SplitPlan {
        postimages: files
            .into_iter()
            .filter(|(path, contents)| {
                snapshot.notes.get(path) != Some(contents)
            })
            .collect(),
        owner_path: request.owner_path.clone(),
        owner_line: owner.line + 1,
        owner_block_id,
        owner_minted_id,
        tasks: planned_tasks,
        stubs,
        placement,
        notice,
        cursor_path: slash_path(&destination_path),
        cursor_line,
        clamped,
    })
}

pub(crate) fn list_task_split_candidates(
    snapshot: &VaultSnapshot,
    request: &SplitRequest,
    task_settings: &NoteTaskSettings,
) -> Result<Vec<SplitCandidate>, SplitError> {
    let source = snapshot.notes.get(&request.owner_path).ok_or_else(|| {
        error(
            "split_owner_missing",
            format!(
                "{} does not exist in the vault",
                request.owner_path.display()
            ),
        )
    })?;
    let owner =
        resolve_owner(source, &request.owner_path, request, task_settings)?;
    Ok(collect_candidates(
        source,
        owner.line,
        owner.end,
        owner.kind,
        task_settings,
    )
    .iter()
    .enumerate()
    .map(|(index, item)| SplitCandidate {
        ordinal: index + 1,
        line: item.line + 1,
        title: item.title.clone(),
        drawer: item.drawer_title.clone(),
        child_lines: item.child_lines.len(),
    })
    .collect())
}

fn resolve_owner(
    source: &str,
    path: &Path,
    request: &SplitRequest,
    settings: &NoteTaskSettings,
) -> Result<SelectedOwner, SplitError> {
    let lines = split_lines(source);
    match &request.selection {
        SplitSelection::Section { title } => {
            let line = lines
                .iter()
                .position(|raw| {
                    heading(raw).is_some_and(|heading| {
                        heading.eq_ignore_ascii_case(title)
                    })
                })
                .ok_or_else(|| {
                    error(
                        "split_section_missing",
                        format!("no ## {title} section in {}", path.display()),
                    )
                })?;
            let end = lines
                .iter()
                .enumerate()
                .skip(line + 1)
                .find_map(|(index, raw)| {
                    heading(raw).is_some().then_some(index - 1)
                })
                .unwrap_or(lines.len() - 1);
            Ok(SelectedOwner {
                kind: OwnerKind::Section,
                line,
                end,
                title: title.clone(),
                block_id: None,
            })
        }
        _ => {
            let settings = settings.clone();
            let scan = note_tasks::scan(source, &settings);
            let line = request.owner_line.saturating_sub(1);
            let Some(task) = scan.task_at(line) else {
                let text = lines.get(line).map(|raw| raw.trim()).unwrap_or("");
                if text.starts_with("- [") && text.contains("#task") {
                    return Err(error("split_already_task", "Already a task — split one of its notes or drawer entries instead"));
                }
                if let Some(owner) = section_owner_for_bullet(&lines, line) {
                    return Ok(owner);
                }
                return Err(error(
                    "split_owner_missing",
                    format!(
                        "no task owner at {}:{}",
                        path.display(),
                        request.owner_line
                    ),
                ));
            };
            Ok(SelectedOwner {
                kind: OwnerKind::Task,
                line,
                end: line_end_index(&lines, task.block_end),
                title: task.description.clone(),
                block_id: task
                    .block_id
                    .clone()
                    .or_else(|| block_id_before_inline_fields(&lines[line])),
            })
        }
    }
}

fn section_owner_for_bullet(
    lines: &[String],
    bullet_line: usize,
) -> Option<SelectedOwner> {
    let bullet = lines.get(bullet_line)?;
    if !indent(strip_ending(bullet)).is_empty()
        || list_body(strip_ending(bullet)).is_none()
    {
        return None;
    }
    let heading_line = (0..bullet_line)
        .rev()
        .find(|index| heading(&lines[*index]).is_some())?;
    let title = heading(&lines[heading_line])?;
    let end = (heading_line + 1..lines.len())
        .find(|index| heading(&lines[*index]).is_some())
        .map(|index| index.saturating_sub(1))
        .unwrap_or_else(|| lines.len().saturating_sub(1));
    Some(SelectedOwner {
        kind: OwnerKind::Section,
        line: heading_line,
        end,
        title,
        block_id: None,
    })
}

fn block_id_before_inline_fields(line: &str) -> Option<String> {
    let captures = BLOCK_ID_BEFORE_INLINE_FIELDS_RE.captures(line)?;
    captures.get(1).map(|id| id.as_str().to_string())
}

fn collect_candidates(
    source: &str,
    owner_line: usize,
    owner_end: usize,
    kind: OwnerKind,
    settings: &NoteTaskSettings,
) -> Vec<Candidate> {
    let lines = split_lines(source);
    let content = lines
        .iter()
        .map(|line| strip_ending(line))
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    if kind == OwnerKind::Task {
        let owner_indent = indent(content[owner_line]);
        let mut drawer_lines = BTreeSet::new();
        for index in
            owner_line + 1..=owner_end.min(content.len().saturating_sub(1))
        {
            if list_body(content[index]).is_none()
                || indent(content[index]).len() <= owner_indent.len()
            {
                continue;
            }
            let classified =
                task_children::classify_task_child(content[index], None);
            if let TaskChildKind::Drawer { .. } = classified {
                let drawer_indent = indent(content[index]).len();
                for entry in
                    index + 1..=owner_end.min(content.len().saturating_sub(1))
                {
                    if !content[entry].trim().is_empty()
                        && indent(content[entry]).len() <= drawer_indent
                    {
                        break;
                    }
                    if list_body(content[entry]).is_some()
                        && indent(content[entry]).len() > drawer_indent
                        && parent_list_line(&content, entry, owner_line)
                            == Some(index)
                    {
                        drawer_lines.insert(entry);
                    }
                }
            }
        }
        for index in
            owner_line + 1..=owner_end.min(content.len().saturating_sub(1))
        {
            let Some(body) = list_body(content[index]) else {
                continue;
            };
            let i = indent(content[index]);
            if i.len() <= owner_indent.len() {
                continue;
            }
            let direct = parent_list_line(&content, index, owner_line);
            let is_direct = direct == Some(owner_line);
            if !is_direct && !drawer_lines.contains(&index) {
                continue;
            }
            let child_kind =
                task_children::classify_task_child(content[index], None);
            let drawer_title = match child_kind {
                TaskChildKind::Drawer { ref title } => Some(title.clone()),
                _ => None,
            };
            if drawer_title.is_some()
                || matches!(
                    child_kind,
                    TaskChildKind::ManagedLink(_)
                        | TaskChildKind::ManagedLog(_)
                        | TaskChildKind::NestedTask { .. }
                )
            {
                continue;
            }
            let (title, _) = parse_bullet(body);
            let title = without_trailing_id(title).to_string();
            let end = list_subtree_end(&content, index, owner_end);
            let child_lines = if end > index {
                lines[index + 1..=end.min(lines.len().saturating_sub(1))]
                    .to_vec()
            } else {
                Vec::new()
            };
            if body.trim_start().starts_with("(**)")
                || body.trim_start().starts_with("(**")
            {
                continue;
            }
            let parent_drawer = if is_direct {
                None
            } else {
                parent_list_line(&content, index, owner_line)
                    .and_then(|parent| list_body(content[parent]))
                    .map(|body| body.trim().to_string())
            };
            result.push(Candidate {
                line: index,
                end,
                indent: i.to_string(),
                raw_body: body.to_string(),
                title,
                child_lines,
                drawer_title: parent_drawer,
            });
        }
    } else {
        for index in
            owner_line + 1..=owner_end.min(content.len().saturating_sub(1))
        {
            let Some(body) = list_body(content[index]) else {
                continue;
            };
            if !indent(content[index]).is_empty() {
                continue;
            }
            let (title, _) = parse_bullet(body);
            let title = without_trailing_id(title).to_string();
            let end = list_subtree_end(&content, index, owner_end);
            let child_lines = if end > index {
                lines[index + 1..=end.min(lines.len().saturating_sub(1))]
                    .to_vec()
            } else {
                Vec::new()
            };
            result.push(Candidate {
                line: index,
                end,
                indent: String::new(),
                raw_body: body.to_string(),
                title,
                child_lines,
                drawer_title: None,
            });
        }
    }
    let _ = settings;
    result
}

fn select_candidates(
    candidates: &[Candidate],
    selection: &SplitSelection,
    source: &str,
    owner: &SelectedOwner,
) -> Result<Vec<usize>, SplitError> {
    let lines = split_lines(source);
    let line_to_candidate = |line: usize| {
        candidates.iter().position(|candidate| {
            line >= candidate.line && line <= candidate.end
        })
    };
    match selection {
        SplitSelection::Cursor { line } => {
            let index = line.saturating_sub(1);
            if owner.kind == OwnerKind::Task && index == owner.line {
                return Err(error("split_already_task", "Already a task — split one of its notes or drawer entries instead"));
            }
            if let Some(candidate) = line_to_candidate(index) {
                return validate_candidates(candidates, vec![candidate]);
            }
            if let Some(raw) = lines.get(index) {
                if raw.contains("DEPENDS ON")
                    || raw.contains("BLOCKS:")
                    || raw.contains("LINKS:")
                    || raw.contains("LINKED FROM")
                    || raw.contains("SCHEDULE LOG")
                    || raw.contains("WORK LOG")
                    || raw.contains("ROTTEN LOG")
                    || raw.contains("CANCEL LOG")
                    || raw.trim_start().starts_with("- (**")
                {
                    return Err(error("split_managed_line", "Managed line — Bob writes it; split a note or drawer entry instead"));
                }
                if raw.trim_start().starts_with("- [x]")
                    || raw.trim_start().starts_with("- [X]")
                {
                    return Err(error(
                        "split_not_splittable",
                        "Checked box — split an unchecked note instead",
                    ));
                }
                if line_to_candidate(index).is_none()
                    && raw.contains("[[")
                    && !raw.contains(" ")
                {
                    return Err(error("split_stub", "Stub only links a task — split a note with its own text instead"));
                }
            }
            Err(error(
                "split_not_splittable",
                "That line is not a splittable note",
            ))
        }
        SplitSelection::Count { line, count } => {
            let start =
                line_to_candidate(line.saturating_sub(1)).ok_or_else(|| {
                    error(
                        "split_not_splittable",
                        "That line is not a splittable note",
                    )
                })?;
            let candidate = &candidates[start];
            let mut siblings = candidates
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.indent == candidate.indent
                        && item.drawer_title == candidate.drawer_title
                        && item.line >= candidate.line
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            siblings.truncate(*count);
            validate_candidates(candidates, siblings)
        }
        SplitSelection::Lines { from, to } => {
            let mut selected = candidates
                .iter()
                .enumerate()
                .filter_map(|(index, candidate)| {
                    ((candidate.line + 1) <= *to
                        && (candidate.end + 1) >= *from)
                        .then_some(index)
                })
                .collect::<Vec<_>>();
            selected.sort_by_key(|index| candidates[*index].line);
            validate_candidates(candidates, selected)
        }
        SplitSelection::Nth(picks) => {
            let mut selected = Vec::new();
            for pick in picks {
                if *pick == 0 || *pick > candidates.len() {
                    return Err(error(
                        "split_nth_missing",
                        format!("no splittable bullet {}", pick),
                    ));
                }
                selected.push(*pick - 1);
            }
            validate_candidates(candidates, selected)
        }
        SplitSelection::Match(texts) => {
            let mut selected = Vec::new();
            for text in texts {
                let matches = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| candidate.title.contains(text))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [index] => selected.push(*index),
                    [] => {
                        return Err(error(
                            "split_match_missing",
                            format!("no splittable bullet contains {text:?}"),
                        ))
                    }
                    _ => {
                        return Err(error(
                            "split_match_ambiguous",
                            format!(
                            "more than one splittable bullet contains {text:?}"
                        ),
                        ))
                    }
                }
            }
            validate_candidates(candidates, selected)
        }
        SplitSelection::Section { .. } => {
            validate_candidates(candidates, (0..candidates.len()).collect())
        }
        SplitSelection::BulletRef(value) => {
            let Some((line, digest)) = value.split_once(':') else {
                return Err(error(
                    "split_stale_ref",
                    "stale bullet reference; list candidates and try again",
                ));
            };
            let Ok(line) = line.parse::<usize>() else {
                return Err(error(
                    "split_stale_ref",
                    "stale bullet reference; list candidates and try again",
                ));
            };
            let Some(index) = candidates.iter().position(|candidate| {
                candidate.line + 1 == line
                    && lines.get(candidate.line).is_some_and(|raw| {
                        note_tasks::line_digest(strip_ending(raw)) == digest
                    })
            }) else {
                return Err(error(
                    "split_stale_ref",
                    "stale bullet reference; list candidates and try again",
                ));
            };
            validate_candidates(candidates, vec![index])
        }
    }
}

fn validate_candidates(
    candidates: &[Candidate],
    selected: Vec<usize>,
) -> Result<Vec<usize>, SplitError> {
    for index in &selected {
        let item = &candidates[*index];
        let (title, checked) = parse_bullet(&item.raw_body);
        if item.raw_body.contains("#task") {
            return Err(error("split_already_task", "Already a task — split one of its notes or drawer entries instead"));
        }
        if checked {
            return Err(error(
                "split_not_splittable",
                "Checked box — split an unchecked note instead",
            ));
        }
        if title.trim().is_empty() {
            return Err(error(
                "split_not_splittable",
                "Bullet has no text to split",
            ));
        }
        if is_stub(title) {
            return Err(error("split_stub", "Stub only links a task — split a note with its own text instead"));
        }
    }
    Ok(selected)
}

fn choose_placement(
    snapshot: &VaultSnapshot,
    source_path: &Path,
    source: &str,
    owner: &SelectedOwner,
    inbox: &str,
) -> SplitPlacement {
    if owner.kind == OwnerKind::Section {
        return SplitPlacement {
            kind: "tasks_section",
            path: slash_path(source_path),
        };
    }
    let lines = split_lines(source);
    let task_line = strip_ending(&lines[owner.line]);
    let daily_owner = source_path.to_string_lossy().contains('/')
        && source_path
            .parent()
            .is_some_and(|parent| parent.to_string_lossy().starts_with("20"))
        || task_line.contains("#gtd");
    let root = inline_field(task_line, "root");
    if daily_owner {
        if let Some(root) = root {
            let path = path_with_md(&root);
            return SplitPlacement {
                kind: "root_note",
                path: slash_path(&path),
            };
        }
        return SplitPlacement {
            kind: "inbox",
            path: slash_path(&path_with_md(inbox)),
        };
    }
    let _ = snapshot;
    SplitPlacement {
        kind: "below_owner",
        path: slash_path(source_path),
    }
}

fn placement_path(
    snapshot: &VaultSnapshot,
    source: &Path,
    placement: &SplitPlacement,
) -> Result<PathBuf, SplitError> {
    match placement.kind {
        "below_owner" | "tasks_section" => Ok(source.to_path_buf()),
        _ => {
            let path = PathBuf::from(&placement.path);
            if snapshot.notes.contains_key(&path) {
                return Ok(path);
            }
            // Routes may be stored without the .md suffix.
            let with_ext = path_with_md(&path.to_string_lossy());
            if snapshot.notes.contains_key(&with_ext) {
                Ok(with_ext)
            } else {
                Ok(with_ext)
            }
        }
    }
}

fn insert_after_block(contents: &mut String, end: usize, generated: &[String]) {
    if generated.is_empty() {
        return;
    }
    let mut lines = split_lines(contents);
    let position = (end + 1).min(lines.len());
    let mut insertion = generated.to_vec();
    if position > 0 && !lines[position - 1].ends_with('\n') {
        lines[position - 1].push('\n');
    }
    lines.splice(position..position, insertion.drain(..));
    *contents = lines.concat();
}

fn insert_tasks_section(
    contents: &mut String,
    generated: &[String],
) -> Result<(), SplitError> {
    *contents = insert_tasks_section_string(contents, generated)?;
    Ok(())
}

fn insert_tasks_section_string(
    contents: &str,
    generated: &[String],
) -> Result<String, SplitError> {
    let mut lines = split_lines(contents);
    let heading_line = lines.iter().position(|line| {
        heading(line).is_some_and(|title| title.eq_ignore_ascii_case("Tasks"))
    });
    match heading_line {
        Some(start) => {
            let end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find_map(|(index, line)| {
                    heading(line).is_some().then_some(index)
                })
                .unwrap_or(lines.len());
            let insertion = (start + 1..end)
                .find(|index| lines[*index].trim().is_empty())
                .unwrap_or(end);
            let mut at = end;
            // Add after existing task rows in this section, keeping the heading's body contiguous.
            while at > start + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            let _ = insertion;
            if at > 0 && !lines[at - 1].ends_with('\n') {
                lines[at - 1].push('\n');
            }
            lines.splice(at..at, generated.iter().cloned());
        }
        None => {
            if !lines.is_empty()
                && !lines.last().unwrap().trim().is_empty()
                && !lines.last().unwrap().ends_with('\n')
            {
                lines.last_mut().unwrap().push('\n');
            }
            if !lines.is_empty() && !lines.last().unwrap().trim().is_empty() {
                lines.push("\n".to_string());
            }
            lines.push("## Tasks\n".to_string());
            lines.extend(generated.iter().cloned());
        }
    }
    Ok(lines.concat())
}

fn rebase_children(
    children: &[String],
    source_indent: &str,
    dest_indent: &str,
) -> Vec<String> {
    children
        .iter()
        .map(|raw| {
            if raw.trim().is_empty() {
                return raw.clone();
            }
            let content = strip_ending(raw);
            let indent = indent(content);
            let rel = indent.strip_prefix(source_indent).unwrap_or("");
            let suffix = raw.strip_prefix(indent).unwrap_or(raw);
            format!("{dest_indent}{rel}{suffix}")
        })
        .collect()
}

fn parse_bullet(body: &str) -> (&str, bool) {
    let trimmed = body.trim();
    if let Some(rest) = trimmed.strip_prefix("[ ] ") {
        return (rest, false);
    }
    if trimmed.starts_with("[x] ") || trimmed.starts_with("[X] ") {
        return (&trimmed[4..], true);
    }
    (trimmed, false)
}

fn without_trailing_id(title: &str) -> &str {
    let title = title.trim_end();
    if let Some(id) = trailing_block_id_in_line(title) {
        let suffix = format!("^{id}");
        return title.strip_suffix(&suffix).unwrap_or(title).trim_end();
    }
    title
}

fn is_stub(title: &str) -> bool {
    let text = title.trim();
    text.starts_with("[[")
        && text.ends_with("]]")
        && !text[2..text.len().saturating_sub(2)].contains(' ')
        || text.starts_with("[[")
            && text.contains("]] ") == false
            && text.ends_with("]]")
}

fn list_body(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("- ")
}

fn indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

fn parent_list_line(
    lines: &[&str],
    index: usize,
    floor: usize,
) -> Option<usize> {
    let current = indent(lines[index]).len();
    (floor..index).rev().find(|prior| {
        list_body(lines[*prior]).is_some()
            && indent(lines[*prior]).len() < current
    })
}

fn list_subtree_end(lines: &[&str], start: usize, limit: usize) -> usize {
    let base = indent(lines[start]).len();
    let mut end = start;
    for index in start + 1..=limit.min(lines.len().saturating_sub(1)) {
        let line = lines[index];
        if line.trim().is_empty() {
            continue;
        }
        if indent(line).len() <= base {
            return end;
        }
        end = index;
    }
    end
}

fn heading(line: &str) -> Option<String> {
    let text = strip_ending(line).trim_start();
    let title = text.strip_prefix("## ")?;
    Some(title.trim().trim_end_matches('#').trim().to_string())
}

fn split_lines(contents: &str) -> Vec<String> {
    if contents.is_empty() {
        return Vec::new();
    }
    contents.split_inclusive('\n').map(str::to_string).collect()
}

fn strip_ending(line: &str) -> &str {
    line.strip_suffix('\n')
        .unwrap_or(line)
        .strip_suffix('\r')
        .unwrap_or(line.strip_suffix('\n').unwrap_or(line))
}

fn split_ending(line: &str) -> (&str, &str) {
    if let Some(text) = line.strip_suffix("\r\n") {
        (text, "\r\n")
    } else if let Some(text) = line.strip_suffix('\n') {
        (text, "\n")
    } else {
        (line, "")
    }
}

fn owner_indent(line: &str) -> String {
    indent(strip_ending(line)).to_string()
}

fn set_trailing_block_id(line: &str, id: &str) -> String {
    if trailing_block_id_in_line(line).is_some() {
        return line.to_string();
    }
    let (content, ending) = split_ending(line);
    format!("{} ^{id}{ending}", content.trim_end())
}

fn line_end_index(lines: &[String], byte_end: usize) -> usize {
    let mut bytes = 0;
    for (index, line) in lines.iter().enumerate() {
        bytes += line.len();
        if bytes >= byte_end {
            return index;
        }
    }
    lines.len().saturating_sub(1)
}

fn locate_line(contents: &str, id: &str) -> Option<usize> {
    split_lines(contents)
        .iter()
        .position(|line| trailing_block_id_in_line(line).as_deref() == Some(id))
}

fn alias_for(title: &str) -> String {
    let text = flatten_links(title)
        .replace("**", "")
        .replace("__", "")
        .replace("==", "")
        .replace("~~", "")
        .replace('`', "")
        .replace('|', "/")
        .replace('[', "")
        .replace(']', "");
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 80 {
        return collapsed;
    }
    let mut out = String::new();
    for word in collapsed.split_whitespace() {
        if out.chars().count()
            + word.chars().count()
            + usize::from(!out.is_empty())
            + 1
            > 80
        {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.push('…');
    out
}

fn short_notice_title(title: &str) -> String {
    let collapsed = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 80 {
        return collapsed;
    }
    let mut out = String::new();
    for word in collapsed.split_whitespace() {
        if out.chars().count()
            + word.chars().count()
            + usize::from(!out.is_empty())
            + 1
            > 80
        {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.push('…');
    out
}

fn flatten_for_id(title: &str) -> String {
    flatten_links(title)
}

fn flatten_links(text: &str) -> String {
    let wiki = Regex::new(r"\[\[([^\]]+)\]\]").unwrap();
    let out = wiki
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let content = caps.get(1).unwrap().as_str();
            let display = content
                .split('|')
                .nth(1)
                .unwrap_or(content.split('#').next().unwrap_or(content));
            display.to_string()
        })
        .to_string();
    let markdown = Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap();
    markdown.replace_all(&out, "$1").to_string()
}

fn inline_field(line: &str, name: &str) -> Option<String> {
    let pattern = Regex::new(&format!(r"\[{name}::\s*([^\]]+)\]")).ok()?;
    pattern
        .captures(line)
        .and_then(|caps| caps.get(1))
        .map(|value| value.as_str().trim().to_string())
}

fn path_with_md(route: &str) -> PathBuf {
    let mut path = PathBuf::from(route);
    if path.extension().is_none() {
        path.set_extension("md");
    }
    path
}

fn note_target(path: &Path) -> String {
    path.with_extension("").to_string_lossy().replace('\\', "/")
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn error(code: &'static str, message: impl Into<String>) -> SplitError {
    SplitError {
        code,
        message: message.into(),
    }
}
