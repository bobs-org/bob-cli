//! `bob task rename-drawer OLD NEW`: rename task drawers and `^prj` `##`
//! drawer sections across the vault, merging them where OLD meets NEW
//! (`docs/task-requirements.md` §3.5, "Renaming drawers").
//!
//! [`rename_note`] is the pure per-note pass. Task drawers (open, closed, or
//! archived) are renamed owner by owner from the bottom of the note up. A
//! rename only rewrites lines inside its own task block, so every task above
//! it keeps its line and each reported task line is the line it holds in the
//! original note. A project note's eligible `##` sections are renamed last;
//! a task-drawer rename never adds or removes a heading. Where one owner
//! holds NEW as well as OLD, or holds OLD twice, the containers merge into
//! one at the earliest participating position, labelled NEW, with every body
//! kept in original order. Nothing else changes: bullets never turn into
//! reqs, and anchors, checkbox bytes, logs, and nesting stay as written.
//!
//! [`links`] repairs heading links into renamed project sections, and
//! [`command`] holds the dry-run report and the one-commit apply.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    capture::{
        document_line_ending, leading_whitespace, line_spans, list_item_body,
        LineSpan,
    },
    capture_drawers::{
        can_create_drawer, drawer_body_title, drawer_key,
        is_reserved_drawer_key, is_structural_h2_key, project_h2_drawers,
        Drawer, DrawerTitleRefusal,
    },
    markdown,
    note_tasks::{self, NoteTask, NoteTaskSettings},
    projects::{frontmatter_is_project, parse_frontmatter},
    task_block_layout::{BlockChild, TaskBlock},
    task_children::TaskChildKind,
};

mod command;
mod links;

#[cfg(test)]
mod tests;

pub(crate) use command::{build_cli, run};

// ---------------------------------------------------------------------------
// The rename request
// ---------------------------------------------------------------------------

/// A validated `OLD NEW` pair. OLD matches by drawer key; NEW is written
/// verbatim as a bold task drawer and a project `##` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameSpec {
    pub(crate) old_input: String,
    pub(crate) new_input: String,
    pub(crate) old_key: String,
    pub(crate) new_key: String,
    /// NEW as a task drawer title.
    pub(crate) drawer_title: String,
    /// NEW as a project `##` section title.
    pub(crate) section_title: String,
    /// OLD's first word, lowercased: a note that never mentions it holds no
    /// OLD container and no link to one.
    needle: String,
}

/// Why an `OLD NEW` pair is refused before anything is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecRefusal {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) hint: &'static str,
}

impl RenameSpec {
    pub(crate) fn parse(old: &str, new: &str) -> Result<Self, SpecRefusal> {
        let old_key = drawer_key(old);
        if old_key.is_empty() {
            return Err(SpecRefusal {
                code: "invalid_drawer_title",
                message: "OLD is empty".to_string(),
                hint: "name the drawer to rename, e.g. REQUIREMENTS",
            });
        }
        if is_reserved_drawer_key(&old_key) {
            return Err(SpecRefusal {
                code: "reserved_drawer",
                message: format!("{old_key} is a managed line, not a drawer"),
                hint: "managed links, logs, and TASKS are never drawers",
            });
        }
        let drawer_title = new.to_string();
        if let Err(reason) = can_create_drawer(&drawer_title, true) {
            let (message, hint) = match reason {
                DrawerTitleRefusal::NotADrawerTitle => (
                    format!("`{new}` is not a valid drawer title"),
                    "a title must be a single line with an alphanumeric character, no * or outer whitespace, and no trailing :",
                ),
                DrawerTitleRefusal::Reserved => (
                    format!(
                        "{} is reserved for a managed or structural line",
                        drawer_key(&drawer_title)
                    ),
                    "choose a title that is not a managed line or structural section",
                ),
                DrawerTitleRefusal::EmptyDrawer => (
                    format!("`{new}` cannot be used for an empty drawer"),
                    "rename preserves the existing drawer content",
                ),
            };
            return Err(SpecRefusal {
                code: reason.code(),
                message,
                hint,
            });
        }
        let new_key = drawer_key(&drawer_title);
        let section_title = drawer_title.clone();
        let needle =
            old_key.split(' ').next().unwrap_or_default().to_lowercase();
        Ok(Self {
            old_input: old.trim().to_string(),
            new_input: new.trim().to_string(),
            old_key,
            new_key,
            drawer_title,
            section_title,
            needle,
        })
    }

    /// Cheap prefilter: whether `contents` can hold an OLD container or a
    /// heading link to one.
    pub(crate) fn may_match(&self, contents: &str) -> bool {
        contents.to_lowercase().contains(&self.needle)
    }

    pub(crate) fn is_old(&self, title: &str) -> bool {
        drawer_key(title) == self.old_key
    }

    pub(crate) fn case_only(&self) -> bool {
        self.old_key == self.new_key
    }

    pub(crate) fn renames_drawer(&self, title: &str) -> bool {
        self.is_old(title) && (!self.case_only() || title != self.drawer_title)
    }

    pub(crate) fn renames_section(&self, title: &str) -> bool {
        self.is_old(title) && (!self.case_only() || title != self.section_title)
    }

    fn participates(&self, key: &str) -> bool {
        key == self.old_key || key == self.new_key
    }
}

// ---------------------------------------------------------------------------
// Per-note outcome
// ---------------------------------------------------------------------------

/// One task whose drawers were renamed, and merged when two or more took
/// part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrawerChange {
    /// 1-based task line, as in the original note.
    pub(crate) line: usize,
    pub(crate) task: String,
    pub(crate) block_id: Option<String>,
    pub(crate) open: bool,
    /// The participating drawer titles in document order; the first one's
    /// position is kept.
    pub(crate) titles: Vec<String>,
    /// How many of them were OLD.
    pub(crate) renamed: usize,
}

/// One project note's renamed `##` sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectionChange {
    /// 1-based line of the kept heading, as in the original note.
    pub(crate) line: usize,
    pub(crate) titles: Vec<String>,
    pub(crate) renamed: usize,
}

/// Merges are counted per owner: containers that became one.
pub(crate) fn merged(titles: &[String]) -> usize {
    titles.len().saturating_sub(1)
}

/// An OLD lookalike the rename leaves as written, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Excluded {
    pub(crate) line: usize,
    pub(crate) text: String,
    pub(crate) reason: &'static str,
}

/// One note's rename outcome. `updated` is `None` when nothing changes or
/// the note is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NoteRename {
    pub(crate) project: bool,
    pub(crate) updated: Option<String>,
    pub(crate) drawers: Vec<DrawerChange>,
    pub(crate) sections: Vec<SectionChange>,
    /// Why the whole note is left unchanged.
    pub(crate) refusal: Option<String>,
    pub(crate) excluded: Vec<Excluded>,
}

impl NoteRename {
    pub(crate) fn renamed(&self) -> usize {
        self.drawers
            .iter()
            .map(|change| change.renamed)
            .sum::<usize>()
            + self
                .sections
                .iter()
                .map(|change| change.renamed)
                .sum::<usize>()
    }

    pub(crate) fn merged(&self) -> usize {
        self.drawers
            .iter()
            .map(|change| merged(&change.titles))
            .sum::<usize>()
            + self
                .sections
                .iter()
                .map(|change| merged(&change.titles))
                .sum::<usize>()
    }

    fn refused(mut self, reason: String) -> Self {
        self.refusal = Some(reason);
        self.updated = None;
        self.drawers.clear();
        self.sections.clear();
        self
    }
}

/// Plan the rename in one note.
pub(crate) fn rename_note(
    contents: &str,
    spec: &RenameSpec,
    settings: &NoteTaskSettings,
) -> NoteRename {
    let mut result = NoteRename::default();
    if !spec.may_match(contents) {
        return result;
    }
    result.project = is_project_text(contents);
    let owners = drawer_owners(contents, spec, settings);
    let sections = if result.project {
        section_participants(contents, spec)
    } else {
        Vec::new()
    };
    let mut participant_lines: BTreeSet<usize> = owners
        .iter()
        .flat_map(|owner| owner.participants.iter().map(|child| child.line))
        .collect();
    participant_lines.extend(sections.iter().map(Drawer::line_index));
    result.excluded =
        excluded_lookalikes(contents, spec, result.project, &participant_lines);
    if owners.is_empty() && sections.is_empty() {
        return result;
    }
    if !sections.is_empty()
        && let Err(reason) = check_sections(contents, spec, &sections)
    {
        return result.refused(reason);
    }
    let mut current = contents.to_string();
    match rename_task_drawers(&mut current, spec, settings) {
        Ok(changes) => result.drawers = changes,
        Err(reason) => return result.refused(reason),
    }
    if !sections.is_empty() {
        match merge_sections(&current, spec) {
            Ok(updated) => current = updated,
            Err(reason) => return result.refused(reason),
        }
        if spec.case_only() {
            result.sections.extend(
                sections
                    .iter()
                    .filter(|section| spec.renames_section(&section.title))
                    .map(|section| SectionChange {
                        line: section.line,
                        titles: vec![section.title.clone()],
                        renamed: 1,
                    }),
            );
        } else {
            let titles: Vec<String> = sections
                .iter()
                .map(|section| section.title.clone())
                .collect();
            result.sections.push(SectionChange {
                line: sections[0].line,
                renamed: titles
                    .iter()
                    .filter(|title| spec.renames_section(title))
                    .count(),
                titles,
            });
        }
    }
    result.updated = (current != contents).then_some(current);
    result
}

fn is_project_text(contents: &str) -> bool {
    parse_frontmatter(contents)
        .is_some_and(|frontmatter| frontmatter_is_project(&frontmatter))
}

// ---------------------------------------------------------------------------
// Task drawers
// ---------------------------------------------------------------------------

struct DrawerOwner {
    task: NoteTask,
    block: TaskBlock,
    /// Drawer children keyed OLD or NEW, in document order.
    participants: Vec<BlockChild>,
}

fn child_drawer_key(child: &BlockChild) -> Option<String> {
    match &child.kind {
        TaskChildKind::Drawer { title } => Some(drawer_key(title)),
        _ => None,
    }
}

/// Lines that look like an OLD drawer bullet: a list item whose whole body
/// is a drawer title keyed OLD.
fn old_bullet_lines(lines: &[LineSpan<'_>], spec: &RenameSpec) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            list_item_body(line.text).is_some_and(|body| {
                drawer_body_title(body).is_some_and(|(title, _)| {
                    let key = drawer_key(title);
                    !is_reserved_drawer_key(&key)
                        && !is_structural_h2_key(&key)
                        && spec.renames_drawer(title)
                })
            })
        })
        .map(|(index, _)| index)
        .collect()
}

/// Every task with an OLD drawer child, in document order.
fn drawer_owners(
    contents: &str,
    spec: &RenameSpec,
    settings: &NoteTaskSettings,
) -> Vec<DrawerOwner> {
    let lines = line_spans(contents);
    let candidates = old_bullet_lines(&lines, spec);
    if candidates.is_empty() {
        return Vec::new();
    }
    let scan = note_tasks::scan(contents, settings);
    let mut owners = Vec::new();
    for task in scan.tasks() {
        // Only a task whose block holds a candidate line can own one.
        let holds_candidate = candidates.iter().any(|&index| {
            index > task.line_index && lines[index].end <= task.block_end
        });
        if !holds_candidate {
            continue;
        }
        let block = TaskBlock::read(contents, task, &settings.global_filter);
        let participants: Vec<BlockChild> = block
            .children
            .iter()
            .filter(|child| {
                child_drawer_key(child)
                    .is_some_and(|key| spec.participates(&key))
            })
            .cloned()
            .collect();
        if participants.iter().any(|child| match &child.kind {
            TaskChildKind::Drawer { title } => spec.renames_drawer(title),
            _ => false,
        }) {
            owners.push(DrawerOwner {
                task: task.clone(),
                block,
                participants,
            });
        }
    }
    owners
}

/// Rename every owner's drawers, bottom-most owner first, and return the
/// changes in document order.
fn rename_task_drawers(
    current: &mut String,
    spec: &RenameSpec,
    settings: &NoteTaskSettings,
) -> Result<Vec<DrawerChange>, String> {
    let mut changes = Vec::new();
    // Owners at or below this line are settled.
    let mut settled_from = usize::MAX;
    loop {
        let owner = drawer_owners(current, spec, settings)
            .into_iter()
            .filter(|owner| owner.task.line_index < settled_from)
            .max_by_key(|owner| owner.task.line_index);
        let Some(owner) = owner else {
            break;
        };
        let lines = line_spans(current);
        let titles: Vec<String> = owner
            .participants
            .iter()
            .filter_map(|child| match &child.kind {
                TaskChildKind::Drawer { title } => Some(title.clone()),
                _ => None,
            })
            .collect();
        let (updated, owner_changes) = if spec.case_only() {
            respell_task_drawers(current, &owner, spec)
        } else {
            (
                merge_task_drawers(current, &lines, &owner, spec)?,
                vec![DrawerChange {
                    line: owner.task.line_index + 1,
                    task: owner.task.description.clone(),
                    block_id: owner.task.block_id.clone(),
                    open: owner.task.status_type.is_open(),
                    renamed: titles
                        .iter()
                        .filter(|title| spec.renames_drawer(title))
                        .count(),
                    titles,
                }],
            )
        };
        changes.extend(owner_changes);
        settled_from = owner.task.line_index;
        *current = updated;
    }
    changes.reverse();
    Ok(changes)
}

/// Respelling is intentionally not a merge: duplicate identities stay as
/// separate bold containers.
fn respell_task_drawers(
    contents: &str,
    owner: &DrawerOwner,
    spec: &RenameSpec,
) -> (String, Vec<DrawerChange>) {
    let mut segments: Vec<String> =
        contents.split_inclusive('\n').map(str::to_string).collect();
    let mut changes = Vec::new();
    for child in &owner.participants {
        let TaskChildKind::Drawer { title } = &child.kind else {
            continue;
        };
        if !spec.renames_drawer(title) {
            continue;
        }
        segments[child.line] =
            retitle_bullet(&segments[child.line], &spec.drawer_title);
        changes.push(DrawerChange {
            line: owner.task.line_index + 1,
            task: owner.task.description.clone(),
            block_id: owner.task.block_id.clone(),
            open: owner.task.status_type.is_open(),
            titles: vec![title.clone()],
            renamed: 1,
        });
    }
    (segments.concat(), changes)
}

/// Rename the first participant to NEW and move every later participant's
/// entries to the end of its body, re-indented to its level.
fn merge_task_drawers(
    contents: &str,
    lines: &[LineSpan<'_>],
    owner: &DrawerOwner,
    spec: &RenameSpec,
) -> Result<String, String> {
    let segments: Vec<&str> = contents.split_inclusive('\n').collect();
    let ending = document_line_ending(contents);
    let kept = &owner.participants[0];
    let kept_indent = leading_whitespace(lines[kept.line].text);
    let kept_end = kept.content_end(lines);
    let mut removed = BTreeSet::new();
    let mut moved = Vec::new();
    for child in &owner.participants[1..] {
        let indent = leading_whitespace(lines[child.line].text);
        let content_end = child.content_end(lines);
        let first = (child.line + 1..=content_end)
            .find(|&index| !lines[index].text.trim().is_empty());
        if let Some(first) = first {
            for (index, segment) in segments
                .iter()
                .enumerate()
                .take(content_end + 1)
                .skip(first)
            {
                let text = lines[index].text;
                let rebased = if indent == kept_indent || text.trim().is_empty()
                {
                    text.to_string()
                } else if let Some(rest) = text.strip_prefix(indent) {
                    format!("{kept_indent}{rest}")
                } else {
                    return Err(format!(
                        "the {} drawer under line {} is indented differently from its {} drawer, so its entries cannot be merged safely",
                        lines[child.line].text.trim().trim_start_matches(['-', '*', '+']).trim(),
                        owner.task.line_index + 1,
                        lines[kept.line].text.trim().trim_start_matches(['-', '*', '+']).trim(),
                    ));
                };
                moved.push(format!(
                    "{rebased}{}",
                    line_ending_of(segment, ending)
                ));
            }
        }
        // The block's last child keeps its trailing blank lines: they part
        // the task block from what follows.
        let remove_end = if child.end >= owner.block.last_line {
            content_end
        } else {
            child.end
        };
        removed.extend(child.line..=remove_end);
    }
    let mut out =
        String::with_capacity(contents.len() + spec.drawer_title.len());
    for (index, segment) in segments.iter().enumerate() {
        if removed.contains(&index) {
            continue;
        }
        if index == kept.line {
            out.push_str(&retitle_bullet(segment, &spec.drawer_title));
        } else {
            out.push_str(segment);
        }
        if index == kept_end && !moved.is_empty() {
            if !out.ends_with('\n') {
                out.push_str(ending);
            }
            for line in &moved {
                out.push_str(line);
            }
        }
    }
    Ok(keep_tail_shape(contents, out))
}

// ---------------------------------------------------------------------------
// Project sections
// ---------------------------------------------------------------------------

/// The note's eligible `##` sections keyed OLD or NEW, when at least one is
/// OLD.
fn section_participants(contents: &str, spec: &RenameSpec) -> Vec<Drawer> {
    let sections: Vec<Drawer> = project_h2_drawers(contents)
        .into_iter()
        .filter(|section| spec.participates(&section.key))
        .collect();
    if sections
        .iter()
        .any(|section| spec.renames_section(&section.title))
    {
        sections
    } else {
        Vec::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Heading {
    line: usize,
    level: usize,
    title: String,
    key: String,
}

impl Heading {
    fn display(&self) -> String {
        format!("{} {}", "#".repeat(self.level), self.title)
    }
}

/// Every ATX heading outside frontmatter and fences, in document order.
fn headings(contents: &str) -> Vec<Heading> {
    let lines = line_spans(contents);
    let texts: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let fenced = markdown::fenced_lines(&texts, 0..texts.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&texts);
    texts
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !fenced.contains(index)
                && frontmatter_end.is_none_or(|end| *index > end)
        })
        .filter_map(|(index, text)| {
            let (level, title) = markdown::atx_heading(text)?;
            Some(Heading {
                line: index,
                level,
                title: title.to_string(),
                key: drawer_key(title),
            })
        })
        .collect()
}

/// The exclusive end line of the section a drawer value spans.
fn section_end(lines: &[LineSpan<'_>], section: &Drawer) -> usize {
    lines
        .iter()
        .take_while(|line| line.end <= section.block_end)
        .count()
}

/// Refuse a note whose rename would leave a heading ambiguous.
fn check_sections(
    contents: &str,
    spec: &RenameSpec,
    sections: &[Drawer],
) -> Result<(), String> {
    if spec.case_only() {
        return Ok(());
    }
    let old_title = sections
        .iter()
        .find(|section| section.key == spec.old_key)
        .map_or(spec.old_key.as_str(), |section| section.title.as_str());
    if is_structural_h2_key(&spec.new_key) {
        return Err(format!(
            "## {} is a structural section, so ## {old_title} cannot become it",
            spec.section_title
        ));
    }
    let all = headings(contents);
    let section_lines: BTreeSet<usize> =
        sections.iter().map(Drawer::line_index).collect();
    for heading in all.iter().filter(|h| !section_lines.contains(&h.line)) {
        if heading.key == spec.old_key {
            return Err(format!(
                "it also has `{}` on line {}, so a link to #{old_title} could not be repaired safely",
                heading.display(),
                heading.line + 1,
            ));
        }
        if heading.key == spec.new_key {
            return Err(format!(
                "it already has `{}` on line {}, so renaming ## {old_title} would leave two {} headings",
                heading.display(),
                heading.line + 1,
                spec.section_title,
            ));
        }
    }
    // Subheadings that would meet under the merged section.
    let lines = line_spans(contents);
    let mut seen: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for section in sections {
        let start = section.line_index();
        let end = section_end(&lines, section);
        let mut stack: Vec<(usize, String)> = Vec::new();
        for heading in all.iter().filter(|h| h.line > start && h.line < end) {
            while stack
                .last()
                .is_some_and(|(level, _)| *level >= heading.level)
            {
                stack.pop();
            }
            stack.push((heading.level, heading.key.clone()));
            let path: Vec<String> =
                stack.iter().map(|(_, key)| key.clone()).collect();
            match seen.get(&path) {
                Some(owner) if *owner != start => {
                    return Err(format!(
                        "merging would put two `{}` headings under ## {}",
                        heading.display(),
                        spec.section_title,
                    ));
                }
                _ => {
                    seen.insert(path, start);
                }
            }
        }
    }
    Ok(())
}

/// Rename the first participating section to NEW and move every later
/// participant's body to the end of it.
fn merge_sections(contents: &str, spec: &RenameSpec) -> Result<String, String> {
    let sections = section_participants(contents, spec);
    let Some(kept) = sections.first() else {
        return Ok(contents.to_string());
    };
    if spec.case_only() {
        return respell_sections(contents, &sections, spec);
    }
    let lines = line_spans(contents);
    let segments: Vec<&str> = contents.split_inclusive('\n').collect();
    let ending = document_line_ending(contents);
    let content = |from: usize, to: usize| -> Option<(usize, usize)> {
        let first = (from..to).find(|&i| !lines[i].text.trim().is_empty())?;
        let last = (from..to)
            .rev()
            .find(|&i| !lines[i].text.trim().is_empty())?;
        Some((first, last))
    };
    let kept_heading = kept.line_index();
    let kept_end = section_end(&lines, kept);
    let mut lead: Vec<String> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    let mut trail: Vec<String> = Vec::new();
    let mut last_body_line: Option<usize> = None;
    match content(kept_heading + 1, kept_end) {
        Some((first, last)) => {
            lead.extend(
                segments[kept_heading + 1..first]
                    .iter()
                    .map(|s| (*s).to_string()),
            );
            body.extend(
                segments[first..=last].iter().map(|s| (*s).to_string()),
            );
            trail.extend(
                segments[last + 1..kept_end]
                    .iter()
                    .map(|s| (*s).to_string()),
            );
            last_body_line = Some(last);
        }
        None => {
            trail.extend(
                segments[kept_heading + 1..kept_end]
                    .iter()
                    .map(|s| (*s).to_string()),
            );
        }
    }
    let mut removed = BTreeSet::new();
    for section in &sections[1..] {
        let heading = section.line_index();
        let end = section_end(&lines, section);
        removed.extend(heading..end);
        let Some((first, last)) = content(heading + 1, end) else {
            continue;
        };
        match last_body_line {
            None => lead = vec![ending.to_string()],
            Some(previous) => {
                let lists = list_item_body(lines[previous].text).is_some()
                    && list_item_body(lines[first].text).is_some();
                if !lists {
                    body.push(ending.to_string());
                }
            }
        }
        body.extend(segments[first..=last].iter().map(|segment| {
            format!(
                "{}{}",
                markdown::split_line_ending(segment).0,
                line_ending_of(segment, ending)
            )
        }));
        last_body_line = Some(last);
    }
    let kept_had_content = content(kept_heading + 1, kept_end).is_some();
    if body.is_empty() {
        // Nothing moved: a plain rename keeps the section's bytes.
        lead.clear();
    } else if !kept_had_content && trail.is_empty() && kept_end < lines.len() {
        // A body moved under an empty heading is set off like the
        // heading's own: a blank line on each side.
        trail.push(ending.to_string());
    }
    // A body taken from a last line without an ending needs one before
    // whatever follows it.
    if let Some(last) = body.last_mut()
        && !last.ends_with('\n')
    {
        last.push_str(ending);
    }
    let mut out =
        String::with_capacity(contents.len() + spec.section_title.len());
    for (index, segment) in segments.iter().enumerate() {
        if removed.contains(&index) {
            continue;
        }
        if index == kept_heading {
            out.push_str(&retitle_heading(segment, &spec.section_title)?);
            if body.is_empty() {
                continue;
            }
            if !out.ends_with('\n') {
                out.push_str(ending);
            }
            for line in lead.iter().chain(&body).chain(&trail) {
                out.push_str(line);
            }
            continue;
        }
        if !body.is_empty() && index > kept_heading && index < kept_end {
            continue;
        }
        out.push_str(segment);
    }
    Ok(keep_tail_shape(contents, out))
}

/// Respelling preserves duplicate section identities and their bodies.
fn respell_sections(
    contents: &str,
    sections: &[Drawer],
    spec: &RenameSpec,
) -> Result<String, String> {
    let mut segments: Vec<String> =
        contents.split_inclusive('\n').map(str::to_string).collect();
    for section in sections {
        if !spec.renames_section(&section.title) {
            continue;
        }
        let index = section.line_index();
        segments[index] =
            retitle_heading(&segments[index], &spec.section_title)?;
    }
    Ok(segments.concat())
}

// ---------------------------------------------------------------------------
// Excluded lookalikes
// ---------------------------------------------------------------------------

enum Lookalike {
    Heading { level: usize },
    Bullet,
}

fn lookalike(text: &str, spec: &RenameSpec) -> Option<Lookalike> {
    if let Some((level, title)) = markdown::atx_heading(text) {
        return spec
            .renames_section(title)
            .then_some(Lookalike::Heading { level });
    }
    let (title, _) = drawer_body_title(list_item_body(text)?)?;
    let key = drawer_key(title);
    (!is_reserved_drawer_key(&key)
        && !is_structural_h2_key(&key)
        && spec.renames_drawer(title))
    .then_some(Lookalike::Bullet)
}

/// The text after any blockquote markers, and whether there were any.
fn unquote(text: &str) -> (bool, &str) {
    let mut rest = text.trim_start();
    let mut quoted = false;
    while let Some(after) = rest.strip_prefix('>') {
        quoted = true;
        rest = after.trim_start();
    }
    (quoted, rest)
}

/// OLD lookalikes that are not renamed, each with the reason.
fn excluded_lookalikes(
    contents: &str,
    spec: &RenameSpec,
    project: bool,
    participants: &BTreeSet<usize>,
) -> Vec<Excluded> {
    let lines = line_spans(contents);
    let texts: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let logical: Vec<&str> = texts
        .iter()
        .map(|text| list_item_body(text).unwrap_or(text))
        .collect();
    let fenced = markdown::fenced_lines(&logical, 0..logical.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&texts);
    let mut excluded = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        if participants.contains(&index)
            || frontmatter_end.is_some_and(|end| index <= end)
        {
            continue;
        }
        let (quoted, body) = unquote(text);
        let Some(kind) = lookalike(body, spec) else {
            continue;
        };
        let reason = if fenced.contains(&index) {
            "inside a code fence"
        } else if quoted {
            "inside a quote"
        } else {
            match kind {
                Lookalike::Heading { .. } if !project => {
                    "an ordinary heading outside a project note"
                }
                Lookalike::Heading { level } if level != 2 => {
                    "not a ## section"
                }
                Lookalike::Heading { .. } => "a structural section",
                Lookalike::Bullet => "not a direct child of a #task",
            }
        };
        excluded.push(Excluded {
            line: index + 1,
            text: text.trim().to_string(),
            reason,
        });
    }
    excluded
}

/// OLD lookalikes in a template note, outside fences: `(line, kind, text)`
/// with kind `heading` or `drawer`. Templates are reported, never edited.
pub(crate) fn template_lookalikes(
    contents: &str,
    spec: &RenameSpec,
) -> Vec<(usize, &'static str, String)> {
    let lines = line_spans(contents);
    let logical: Vec<&str> = lines
        .iter()
        .map(|line| list_item_body(line.text).unwrap_or(line.text))
        .collect();
    let fenced = markdown::fenced_lines(&logical, 0..logical.len());
    lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !fenced.contains(index))
        .filter_map(|(index, line)| {
            let kind = match lookalike(line.text, spec)? {
                Lookalike::Heading { .. } => "heading",
                Lookalike::Bullet => "drawer",
            };
            Some((index + 1, kind, line.text.trim().to_string()))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Line helpers and safety checks
// ---------------------------------------------------------------------------

/// The byte offset of `inner` inside `outer`; `inner` must be a subslice.
fn offset_in(outer: &str, inner: &str) -> usize {
    let offset =
        (inner.as_ptr() as usize).wrapping_sub(outer.as_ptr() as usize);
    debug_assert!(offset + inner.len() <= outer.len());
    offset
}

fn line_ending_of<'a>(segment: &'a str, fallback: &'a str) -> &'a str {
    match markdown::split_line_ending(segment).1 {
        "" => fallback,
        ending => ending,
    }
}

/// A drawer bullet with its title replaced, every other byte kept.
fn retitle_bullet(segment: &str, title: &str) -> String {
    let (text, ending) = markdown::split_line_ending(segment);
    let Some(body) = list_item_body(text) else {
        return segment.to_string();
    };
    let old = body.trim();
    let start = offset_in(text, old);
    format!(
        "{}**{title}**{}{ending}",
        &text[..start],
        &text[start + old.len()..]
    )
}

/// A heading with its title replaced, every other byte kept.
fn retitle_heading(segment: &str, title: &str) -> Result<String, String> {
    let (text, ending) = markdown::split_line_ending(segment);
    let Some((_, old)) = markdown::atx_heading(text) else {
        return Err(format!("`{}` is no longer a heading", text.trim()));
    };
    let start = offset_in(text, old);
    Ok(format!(
        "{}{title}{}{ending}",
        &text[..start],
        &text[start + old.len()..]
    ))
}

/// Never leave more trailing blank lines, or a final line ending, than the
/// note had: removing a last container must not grow the note's tail.
fn keep_tail_shape(original: &str, updated: String) -> String {
    let blank_tail = |text: &str| {
        text.split_inclusive('\n')
            .rev()
            .take_while(|segment| segment.trim().is_empty())
            .count()
    };
    let excess = blank_tail(&updated).saturating_sub(blank_tail(original));
    let mut out = if excess == 0 {
        updated
    } else {
        let segments: Vec<&str> = updated.split_inclusive('\n').collect();
        segments[..segments.len() - excess].concat()
    };
    if !original.ends_with('\n') && out.ends_with('\n') {
        let trimmed = out.trim_end_matches('\n').trim_end_matches('\r').len();
        out.truncate(trimmed);
    }
    out
}

/// Whether a trimmed line is a container header, and whether it is OLD.
fn header_kind(line: &str, spec: &RenameSpec) -> Option<bool> {
    let (title, is_section) = match markdown::atx_heading(line) {
        Some((2, title)) => (title, true),
        Some(_) => return None,
        None => {
            let (title, _) = drawer_body_title(list_item_body(line)?)?;
            let key = drawer_key(title);
            // Project H2 validation handles structural names separately. A
            // task drawer rename may target one of those spellings, so the
            // content-preservation check must still recognize its output.
            if is_reserved_drawer_key(&key) {
                return None;
            }
            (title, false)
        }
    };
    let key = drawer_key(title);
    let output_title = if is_section {
        &spec.section_title
    } else {
        &spec.drawer_title
    };
    if spec.case_only() && title == output_title {
        Some(false)
    } else if key == spec.old_key {
        Some(true)
    } else if key == spec.new_key {
        Some(false)
    } else {
        None
    }
}

/// Check that a rename only retitled, moved, and re-indented lines: every
/// nonblank line that is not an OLD or NEW header survives exactly once
/// (indentation aside), the renamed OLD headers are gone, and the merged
/// ones became one.
pub(crate) fn content_preserved(
    before: &str,
    after: &str,
    spec: &RenameSpec,
    pass: &NoteRename,
) -> Result<(), String> {
    let mut balance: BTreeMap<&str, isize> = BTreeMap::new();
    let mut headers = [[0isize; 2]; 2];
    for (side, text, sign) in [(0, before, 1isize), (1, after, -1isize)] {
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty())
        {
            match header_kind(line, spec) {
                Some(old) => headers[side][usize::from(old)] += 1,
                None => *balance.entry(line).or_default() += sign,
            }
        }
    }
    for (line, count) in balance {
        if count > 0 {
            return Err(format!("it would drop the line {line:?}"));
        }
        if count < 0 {
            return Err(format!("it would add the line {line:?}"));
        }
    }
    let [[new_before, old_before], [new_after, old_after]] = headers;
    let renamed = pass.renamed() as isize;
    let merged = pass.merged() as isize;
    if old_after != old_before - renamed {
        return Err(format!(
            "it would leave {old_after} {} headers, not {}",
            spec.old_key,
            old_before - renamed
        ));
    }
    if new_after + old_after != new_before + old_before - merged {
        return Err(format!(
            "it would write {} drawer headers, not {}",
            new_after + old_after,
            new_before + old_before - merged
        ));
    }
    Ok(())
}

/// Whether `text` is a settled rename: a second pass changes nothing.
pub(crate) fn settled(
    text: &str,
    spec: &RenameSpec,
    settings: &NoteTaskSettings,
) -> Result<(), String> {
    let pass = rename_note(text, spec, settings);
    if let Some(reason) = pass.refusal {
        return Err(format!("a second pass would refuse it: {reason}"));
    }
    match (pass.drawers.first(), pass.sections.first()) {
        (Some(change), _) => Err(format!(
            "a second pass would still rename the drawer under line {}",
            change.line
        )),
        (None, Some(change)) => Err(format!(
            "a second pass would still rename the section on line {}",
            change.line
        )),
        (None, None) => Ok(()),
    }
}
