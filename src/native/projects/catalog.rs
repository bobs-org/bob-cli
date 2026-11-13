//! Pure sub-project catalog planner (`## Projects` host catalog).
//!
//! Nothing here reads the filesystem, takes a lock, writes, commits, or
//! prints. A caller feeds scanned area/project notes with their contents,
//! a vault [`NoteIndex`], and `today`, and receives per-host plans with
//! before/after bytes plus the child sets sync's `#hide` rule needs.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
};

use chrono::NaiveDate;

use super::catalog_rows::{render_row, CatalogRowState, CLOSED_GROUP_LABEL};
use super::*;
use crate::native::host_catalog::{
    self, EditState, FileEdit, HostOutcome, PROJECTS,
};
use crate::native::notebooks::catalog::inert_text;
use crate::native::project_dependencies::resolve_project_parent;
use crate::native::ref_tasks::close_date;
use crate::native::task_dependencies::parse::{
    dependency_child_of, DependencyLine,
};
use crate::native::vault_links::NoteIndex;

/// Whether a catalog host is an area note or a project note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum HostKind {
    Area,
    Project,
}

impl HostKind {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Area => "area",
            Self::Project => "project",
        }
    }
}

/// How a host's generated `## Projects` catalog compares with the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum CatalogState {
    Current,
    Missing,
    Stale,
    Damaged,
}

impl CatalogState {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Damaged => "damaged",
        }
    }
}

/// One finding on a host's catalog: a stale or damaged region.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CatalogIssue {
    pub(crate) code: &'static str,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

/// One direct child project listed in its parent's catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CatalogChild {
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    /// Vault link inner text: the stem when it resolves uniquely, else
    /// `path-without-md|stem`.
    pub(crate) link: String,
    pub(crate) state: CatalogRowState,
    /// Cleaned criterion, or `None` when the tail is omitted.
    pub(crate) criterion: Option<String>,
    /// Closure date for closed rows, parsed with the shared task parsers.
    pub(crate) closed_on: Option<NaiveDate>,
    pub(crate) closed_on_raw: Option<String>,
    /// 1-based `^prj` line, or 0 when the child has no single `^prj`.
    pub(crate) prj_line: usize,
    pub(crate) prj_mark: char,
    pub(crate) prj_hidden: bool,
    pub(crate) prj_blocked_tag: bool,
    pub(crate) effective_scheduled: Option<NaiveDate>,
    pub(crate) effective_scheduled_raw: Option<String>,
    /// Depends-On targets from the `^prj` line, as `target#^block` or
    /// `#^block` for same-note links.
    pub(crate) waits_on: Vec<String>,
}

impl CatalogChild {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_open(&self) -> bool {
        self.state.is_open()
    }
}

/// One host's planned catalog: record, row counts, and the installing edit.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct HostCatalogPlan {
    pub(crate) host: PathBuf,
    pub(crate) stem: String,
    pub(crate) kind: HostKind,
    pub(crate) state: CatalogState,
    pub(crate) open: usize,
    pub(crate) blocked: usize,
    pub(crate) warning: usize,
    pub(crate) closed: usize,
    pub(crate) rows: Vec<CatalogChild>,
    pub(crate) issues: Vec<CatalogIssue>,
    pub(crate) edit: Option<FileEdit>,
}

/// Every host with expected rows or an existing managed region.
#[derive(Debug, Clone, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CatalogPlan {
    pub(crate) hosts: Vec<HostCatalogPlan>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CatalogPlan {
    pub(crate) fn host(&self, path: &Path) -> Option<&HostCatalogPlan> {
        self.hosts.iter().find(|host| host.host == path)
    }

    pub(crate) fn edits(&self) -> Vec<&FileEdit> {
        self.hosts
            .iter()
            .filter_map(|host| host.edit.as_ref())
            .collect()
    }

    /// Host path to direct-child paths in catalog order, for sync's
    /// `#hide` rule (any open row, warnings included, keeps it hidden).
    pub(crate) fn children_by_host(&self) -> HashMap<PathBuf, Vec<PathBuf>> {
        self.hosts
            .iter()
            .map(|host| {
                (
                    host.host.clone(),
                    host.rows.iter().map(|row| row.path.clone()).collect(),
                )
            })
            .collect()
    }
}

/// Plan every host catalog from scanned notes and the vault link index.
///
/// `notes` maps vault-relative paths to note bytes; only area and project
/// notes outside excluded directories and the `done/` archive can host,
/// and only project notes outside them can be children. Membership
/// resolves each child's scalar `parent:` through `index` with the same
/// exact-path/unique-basename rules the blocking cascade uses.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_catalogs(
    notes: &BTreeMap<PathBuf, String>,
    index: &NoteIndex,
    today: NaiveDate,
) -> CatalogPlan {
    struct HostEntry {
        kind: HostKind,
        stem: String,
        contents: String,
    }

    let mut hosts: BTreeMap<PathBuf, HostEntry> = BTreeMap::new();
    for (path, contents) in notes {
        if !is_plannable_path(path) {
            continue;
        }
        let Some(frontmatter) = parse_frontmatter(contents) else {
            continue;
        };
        let kind = if frontmatter_is_area(&frontmatter) {
            HostKind::Area
        } else if frontmatter_is_project(&frontmatter) {
            HostKind::Project
        } else {
            continue;
        };
        hosts.insert(
            path.clone(),
            HostEntry {
                kind,
                stem: stem_of(path),
                contents: contents.clone(),
            },
        );
    }

    let mut children: BTreeMap<PathBuf, Vec<CatalogChild>> = BTreeMap::new();
    for (path, contents) in notes {
        if !is_plannable_path(path) {
            continue;
        }
        let Some(frontmatter) = parse_frontmatter(contents) else {
            continue;
        };
        if !frontmatter_is_project(&frontmatter) {
            continue;
        }
        let mut issues = Vec::new();
        let Some(project) = parse_project(path, contents, &mut issues) else {
            continue;
        };
        let Some(parent) = resolve_project_parent(path, contents, index) else {
            continue;
        };
        if !hosts.contains_key(&parent) {
            continue;
        }
        let child = child_info(path, contents, &project, index, today);
        children.entry(parent).or_default().push(child);
    }

    let mut plans = Vec::new();
    for (path, entry) in &hosts {
        let rows = children.remove(path).unwrap_or_default();
        let planned = plan_host_catalog(
            path,
            &entry.stem,
            entry.kind,
            &entry.contents,
            rows,
        );
        // Hosts with no rows and no managed content stay out of the plan.
        if planned.rows.is_empty()
            && matches!(planned.state, CatalogState::Current)
            && planned.edit.is_none()
        {
            continue;
        }
        plans.push(planned);
    }
    plans.sort_by(|left, right| left.host.cmp(&right.host));
    CatalogPlan { hosts: plans }
}

/// Plan one host's catalog region from unsorted child rows.
///
/// Legacy marker removal composes with the region write in one postimage;
/// a damaged host returns its text untouched with no edit so readers keep
/// the legacy line as their fallback.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_host_catalog(
    host: &Path,
    stem: &str,
    kind: HostKind,
    contents: &str,
    mut rows: Vec<CatalogChild>,
) -> HostCatalogPlan {
    sort_rows(&mut rows);
    let (open, blocked, warning, closed) = count_rows(&rows);
    let callout = render_callout(&rows);
    let cleaned = remove_legacy_subprojects_lines(contents);
    let planned = host_catalog::plan_host(&PROJECTS, &cleaned, &callout);
    let path = display_slash(host);
    let mut record = HostCatalogPlan {
        host: host.to_path_buf(),
        stem: stem.to_string(),
        kind,
        state: CatalogState::Current,
        open,
        blocked,
        warning,
        closed,
        rows,
        issues: Vec::new(),
        edit: None,
    };
    match planned {
        HostOutcome::Nothing => {
            if cleaned != contents {
                record.state = CatalogState::Stale;
                record.issues.push(CatalogIssue {
                    code: "projects_catalog_stale",
                    line: None,
                    message: format!(
                        "the generated projects catalog in {path} is stale; run `bob projects sync`"
                    ),
                });
                record.edit = Some(FileEdit {
                    path: host.to_path_buf(),
                    before: contents.to_string(),
                    after: cleaned,
                });
            }
        }
        HostOutcome::Current => {
            if cleaned != contents {
                record.state = CatalogState::Stale;
                record.issues.push(CatalogIssue {
                    code: "projects_catalog_stale",
                    line: None,
                    message: format!(
                        "the generated projects catalog in {path} is stale; run `bob projects sync`"
                    ),
                });
                record.edit = Some(FileEdit {
                    path: host.to_path_buf(),
                    before: contents.to_string(),
                    after: cleaned,
                });
            }
        }
        HostOutcome::Update { state, after } => {
            record.state = match state {
                EditState::Missing => CatalogState::Missing,
                EditState::Stale => CatalogState::Stale,
            };
            record.issues.push(CatalogIssue {
                code: "projects_catalog_stale",
                line: None,
                message: format!(
                    "the generated projects catalog in {path} is {}; run `bob projects sync`",
                    record.state.as_str()
                ),
            });
            // `after` is planned from the legacy-cleaned text; when legacy
            // removal changed bytes the postimage already composes both.
            let combined = if cleaned == contents {
                after
            } else if after == cleaned {
                cleaned
            } else {
                after
            };
            record.edit = Some(FileEdit {
                path: host.to_path_buf(),
                before: contents.to_string(),
                after: combined,
            });
        }
        HostOutcome::Damaged(damage) => {
            record.state = CatalogState::Damaged;
            record.issues.push(CatalogIssue {
                code: "projects_catalog_damaged",
                line: Some(damage.line),
                message: format!("{path}: {}", damage.message),
            });
        }
    }
    record
}

/// Delete every direct-child Sub-projects marker line under the single
/// `^prj`, with each marker's subtree, as byte spans. With zero or
/// several `^prj` lines the text is returned unchanged.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn remove_legacy_subprojects_lines(text: &str) -> String {
    let frontmatter = parse_frontmatter(text);
    let body_start = frontmatter.map_or(0, |front| front.body_start_line);
    let lines = line_spans(text);
    let mut fence = None;
    let mut prj_indexes = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.line_number <= body_start {
            continue;
        }
        let line_text = trim_cr(&text[line.start..line.end]);
        if markdown_fence_line(line_text, &mut fence) {
            continue;
        }
        if has_trailing_prj_anchor(line_text) {
            prj_indexes.push(index);
        }
    }
    if prj_indexes.len() != 1 {
        return text.to_string();
    }
    let block = parse_prj_sub_block(text, &lines, prj_indexes[0]);
    let mut ranges = Vec::new();
    for marker in block
        .lines
        .iter()
        .filter(|line| line.is_marker)
        .filter(|line| is_direct_prj_child(&block, line))
    {
        let end = subprojects_subtree_end(&block, marker);
        let Ok(start_line) = line_by_number(&lines, marker.line_number) else {
            continue;
        };
        let Ok(end_line) = line_by_number(&lines, end.line_number) else {
            continue;
        };
        ranges.push(start_line.start..end_line.next_start);
    }
    if ranges.is_empty() {
        return text.to_string();
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    let mut after = text.to_string();
    for range in merged.into_iter().rev() {
        after.replace_range(range, "");
    }
    after
}

fn is_plannable_path(path: &Path) -> bool {
    if !is_markdown_file(path) {
        return false;
    }
    if crate::native::done_archive::is_archive_path(path) {
        return false;
    }
    if path.components().any(|component| match component {
        Component::Normal(name) => {
            crate::native::is_always_excluded_note_directory_name(name)
        }
        _ => false,
    }) {
        return false;
    }
    true
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

fn display_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The vault link inner text for a child: its stem in original case when
/// that resolves uniquely, else `path-without-md|stem`.
fn link_inner(index: &NoteIndex, path: &Path, stem: &str) -> String {
    if index.resolve(None, stem).as_deref() == Some(path) {
        return stem.to_string();
    }
    let mut without_extension = path.to_path_buf();
    without_extension.set_extension("");
    let slashed = display_slash(&without_extension);
    format!("{slashed}|{stem}")
}

pub(super) fn child_info(
    path: &Path,
    contents: &str,
    project: &Project,
    index: &NoteIndex,
    today: NaiveDate,
) -> CatalogChild {
    let stem = stem_of(path);
    let link = link_inner(index, path, &stem);
    let mut effective_status = project.status.clone();
    if let Some(target) = project.prj_task.target_status(&project.status)
        && !target.matches(&project.status)
    {
        effective_status = target.as_project_status();
    }
    let schedule = project_schedule_for_sync(project, &effective_status);
    let raw_prj = single_prj_line(contents);
    let prj_line = project
        .task_lines
        .iter()
        .find(|task| task.is_prj)
        .map(|task| task.line_number)
        .unwrap_or(0);
    let prj_mark = project
        .task_lines
        .iter()
        .find(|task| task.is_prj)
        .map(|task| task.mark)
        .unwrap_or(' ');
    let prj_hidden = project.prj_task.hidden;
    let prj_blocked_tag = raw_prj.as_ref().is_some_and(|(_, line)| {
        line.split_whitespace()
            .any(|token| token == PROJECT_BLOCKED_TAG)
    });
    let waits_on = raw_prj_line_waits_on(contents, raw_prj.as_ref());

    let state = match SubprojectState::from_project(project) {
        Some(SubprojectState::Done) => CatalogRowState::Done,
        Some(SubprojectState::Canceled) => CatalogRowState::Canceled,
        Some(SubprojectState::Open) => {
            if schedule.as_ref().is_some_and(|sched| sched.date > today) {
                CatalogRowState::Scheduled
            } else if prj_mark == '?' || prj_blocked_tag {
                CatalogRowState::Blocked
            } else {
                CatalogRowState::Open
            }
        }
        None => CatalogRowState::Warning,
    };
    let (criterion, closed_on, closed_on_raw) =
        if state == CatalogRowState::Warning {
            (Some("no valid `^prj` task".to_string()), None, None)
        } else {
            let criterion = raw_prj.as_ref().and_then(|(_, line)| {
                let task = parse_task_line(line)?;
                clean_criterion(task.text, &stem)
            });
            let (closed_on, closed_on_raw) = if state.is_open() {
                (None, None)
            } else {
                closed_on_of(raw_prj.as_ref().map(|(_, line)| line.as_str()))
            };
            (criterion, closed_on, closed_on_raw)
        };
    CatalogChild {
        path: path.to_path_buf(),
        stem,
        link,
        state,
        criterion,
        closed_on,
        closed_on_raw,
        prj_line,
        prj_mark,
        prj_hidden,
        prj_blocked_tag,
        effective_scheduled: schedule.as_ref().map(|sched| sched.date),
        effective_scheduled_raw: schedule.map(|sched| sched.raw),
        waits_on,
    }
}

/// The single `^prj` line outside frontmatter and fences, or `None` with
/// zero or several candidates.
fn single_prj_line(contents: &str) -> Option<(usize, String)> {
    let frontmatter = parse_frontmatter(contents);
    let body_start = frontmatter.map_or(0, |front| front.body_start_line);
    let lines = line_spans(contents);
    let mut fence = None;
    let mut found = Vec::new();
    for line in &lines {
        if line.line_number <= body_start {
            continue;
        }
        let line_text = trim_cr(&contents[line.start..line.end]);
        if markdown_fence_line(line_text, &mut fence) {
            continue;
        }
        if has_trailing_prj_anchor(line_text) {
            found.push((line.line_number, line_text.to_string()));
        }
    }
    if found.len() == 1 {
        found.into_iter().next()
    } else {
        None
    }
}

fn raw_prj_line_waits_on(
    contents: &str,
    raw_prj: Option<&(usize, String)>,
) -> Vec<String> {
    let Some((line_number, _)) = raw_prj else {
        return Vec::new();
    };
    let split: Vec<&str> = contents.split('\n').collect();
    let index = line_number.saturating_sub(1);
    if index >= split.len() {
        return Vec::new();
    }
    let mut fenced = BTreeSet::new();
    let mut fence = None;
    for (offset, line) in split.iter().enumerate() {
        let trimmed = line.strip_suffix('\r').unwrap_or(line);
        if markdown_fence_line(trimmed, &mut fence) || fence.is_some() {
            fenced.insert(offset);
        }
    }
    let Some(child) = dependency_child_of(&split, &fenced, index) else {
        return Vec::new();
    };
    match child.parsed {
        DependencyLine::Accepted { links, .. } => links
            .into_iter()
            .map(|link| {
                if link.target.is_empty() {
                    format!("#^{}", link.block_id)
                } else {
                    format!("{}#^{}", link.target, link.block_id)
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn closed_on_of(raw_line: Option<&str>) -> (Option<NaiveDate>, Option<String>) {
    let Some(line) = raw_line else {
        return (None, None);
    };
    let Some(raw) = close_date(line) else {
        return (None, None);
    };
    let value = raw.trim().to_string();
    let date = parse_inline_schedule_date(&value);
    (date, Some(value))
}

/// Clean one `^prj` task text into its catalog criterion, in the contract
/// order: drop tags, drop inline fields plus Tasks emoji dates plus the
/// Keep link plus the trailing block ID, reduce links, collapse, inert.
/// Returns `None` when the tail is omitted.
fn clean_criterion(raw_text: &str, stem: &str) -> Option<String> {
    let without_fields = remove_inline_fields(raw_text);
    let without_keep = remove_keep_links(&without_fields);
    let without_emoji_dates = remove_tasks_emoji_dates(&without_keep);
    let without_block = strip_trailing_block_id(&without_emoji_dates);
    let without_tags: Vec<&str> = without_block
        .split_whitespace()
        .filter(|token| !token.starts_with('#'))
        .collect();
    let joined = without_tags.join(" ");
    let reduced = reduce_links(&joined);
    let collapsed = reduced.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty()
        || collapsed == PLACEHOLDER_CRITERIA
        || collapsed.eq_ignore_ascii_case(stem)
    {
        return None;
    }
    Some(inert_text(&collapsed))
}

fn remove_keep_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("[💡](") {
        out.push_str(&rest[..open]);
        let after = &rest[open + "[💡](".len()..];
        if let Some(close) = after.find(')') {
            rest = &after[close + 1..];
        } else {
            out.push_str(&rest[open..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn remove_tasks_emoji_dates(text: &str) -> String {
    const MARKERS: &[char] = &[
        '🔁', '⏫', '🔼', '🔽', '🔺', '⏬', '🛫', '➕', '⏳', '⌛', '📅', '📆',
        '🗓', '✅', '❌', '🏁', '🆔', '⛔',
    ];
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if MARKERS.contains(&chars[index]) {
            let mut cursor = index + 1;
            if chars.get(cursor) == Some(&'\u{fe0f}') {
                cursor += 1;
            }
            while chars.get(cursor).is_some_and(|ch| ch.is_whitespace()) {
                cursor += 1;
            }
            if let Some(width) = read_date_shape(&chars[cursor..]) {
                // Later whitespace collapsing heals the gap left behind.
                index = cursor + width;
                continue;
            }
            // A bare marker without a date stays; only date fields drop.
            out.push(chars[index]);
            index += 1;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn read_date_shape(chars: &[char]) -> Option<usize> {
    if chars.len() < 10 {
        return None;
    }
    for (offset, ch) in chars.iter().take(10).enumerate() {
        match offset {
            4 | 7 => {
                if *ch != '-' {
                    return None;
                }
            }
            _ => {
                if !ch.is_ascii_digit() {
                    return None;
                }
            }
        }
    }
    Some(10)
}

fn reduce_links(text: &str) -> String {
    let without_markdown = reduce_markdown_links(text);
    reduce_wikilinks(&without_markdown)
}

fn reduce_markdown_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        // Skip wikilinks and inline fields; they are handled elsewhere.
        if rest[open..].starts_with("[[") {
            out.push_str(&rest[..open + 2]);
            rest = &rest[open + 2..];
            continue;
        }
        let Some(close) = rest[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close;
        let label = &rest[open + 1..close];
        if label.contains("::") || label.contains('[') || label.contains(']') {
            out.push_str(&rest[..close + 1]);
            rest = &rest[close + 1..];
            continue;
        }
        let after = &rest[close + 1..];
        if !after.starts_with('(') {
            out.push_str(&rest[..close + 1]);
            rest = &rest[close + 1..];
            continue;
        }
        let Some(paren) = after.find(')') else {
            break;
        };
        let target = after[1..paren].trim();
        if target.is_empty()
            || target.contains(' ')
            || target.contains('\t')
            || target.contains('(')
        {
            // Not a URL-like target; leave the brackets alone.
            out.push_str(&rest[..close + 1]);
            rest = &rest[close + 1..];
            continue;
        }
        out.push_str(label);
        rest = &after[paren + 1..];
    }
    out.push_str(rest);
    out
}

fn reduce_wikilinks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let inner = &after_open[..close];
        if inner.contains("[[") || inner.contains("]]") {
            out.push_str(&rest[..open + 2]);
            rest = after_open;
            continue;
        }
        let before_alias =
            inner.split_once('|').map_or(inner, |(target, _)| target);
        let alias = inner.split_once('|').map(|(_, alias)| alias);
        let replacement = if let Some(alias) = alias {
            alias.trim().to_string()
        } else {
            before_alias
                .split_once('#')
                .map_or(before_alias, |(target, _)| target)
                .rsplit('/')
                .next()
                .unwrap_or(before_alias)
                .trim()
                .to_string()
        };
        out.push_str(&rest[..open]);
        out.push_str(&replacement);
        rest = &after_open[close + 2..];
    }
    out.push_str(rest);
    out
}

fn sort_rows(rows: &mut [CatalogChild]) {
    rows.sort_by(|left, right| {
        match (left.state.is_open(), right.state.is_open()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => left
                .stem
                .to_lowercase()
                .cmp(&right.stem.to_lowercase())
                .then_with(|| left.path.cmp(&right.path)),
            (false, false) => match (&left.closed_on, &right.closed_on) {
                (Some(left_date), Some(right_date)) => right_date
                    .cmp(left_date)
                    .then_with(|| {
                        left.stem.to_lowercase().cmp(&right.stem.to_lowercase())
                    })
                    .then_with(|| left.path.cmp(&right.path)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left
                    .stem
                    .to_lowercase()
                    .cmp(&right.stem.to_lowercase())
                    .then_with(|| left.path.cmp(&right.path)),
            },
        }
    });
}

fn count_rows(rows: &[CatalogChild]) -> (usize, usize, usize, usize) {
    let mut open = 0;
    let mut blocked = 0;
    let mut warning = 0;
    let mut closed = 0;
    for row in rows {
        match row.state {
            CatalogRowState::Open => open += 1,
            CatalogRowState::Scheduled | CatalogRowState::Blocked => {
                blocked += 1;
            }
            CatalogRowState::Warning => warning += 1,
            CatalogRowState::Done | CatalogRowState::Canceled => closed += 1,
        }
    }
    (open, blocked, warning, closed)
}

fn render_callout(rows: &[CatalogChild]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![">".to_string()];
    let mut closed_pushed = false;
    for row in rows {
        let rendered =
            render_row(&row.link, row.state, row.criterion.as_deref());
        if row.state.is_open() {
            lines.push(rendered);
        } else {
            if !closed_pushed {
                lines.push(format!("> - {CLOSED_GROUP_LABEL}"));
                closed_pushed = true;
            }
            lines.push(format!(">   - {}", &rendered[4..]));
        }
    }
    crate::native::host_catalog::canonical_callout(&PROJECTS, &lines)
}

#[cfg(test)]
mod tests {
    use super::catalog_rows::{parse_catalog, CatalogRead};
    use super::*;

    const TODAY: NaiveDate = match NaiveDate::from_ymd_opt(2026, 11, 13) {
        Some(date) => date,
        None => panic!("fixed test date"),
    };

    fn notes(pairs: &[(&str, &str)]) -> BTreeMap<PathBuf, String> {
        pairs
            .iter()
            .map(|(path, contents)| {
                (PathBuf::from(path), (*contents).to_string())
            })
            .collect()
    }

    fn index_of(map: &BTreeMap<PathBuf, String>) -> NoteIndex {
        NoteIndex::from_paths(map.keys().cloned())
    }

    fn project_note(status: &str, parent: Option<&str>, prj: &str) -> String {
        let parent_line = parent
            .map(|target| format!("parent: \"[[{target}]]\"\n"))
            .unwrap_or_default();
        format!(
            "---\ntype: \"[[project]]\"\nstatus: {status}\n{parent_line}---\n{prj}\n"
        )
    }

    fn area_note() -> String {
        "---\ntype: \"[[area]]\"\n---\n# Area\n".to_string()
    }

    fn child(
        stem: &str,
        state: CatalogRowState,
        closed_on: Option<&str>,
    ) -> CatalogChild {
        CatalogChild {
            path: PathBuf::from(format!("{stem}.md")),
            stem: stem.to_string(),
            link: stem.to_string(),
            state,
            criterion: Some(format!("Criterion {stem}")),
            closed_on: closed_on.and_then(|raw| {
                NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
            }),
            closed_on_raw: closed_on.map(str::to_string),
            prj_line: 5,
            prj_mark: ' ',
            prj_hidden: false,
            prj_blocked_tag: false,
            effective_scheduled: None,
            effective_scheduled_raw: None,
            waits_on: Vec::new(),
        }
    }

    #[test]
    fn every_rendered_row_parses_back_to_its_key_and_state() {
        let cases = [
            (CatalogRowState::Open, "alpha"),
            (CatalogRowState::Scheduled, "bravo"),
            (CatalogRowState::Blocked, "charlie"),
            (CatalogRowState::Warning, "delta"),
            (CatalogRowState::Done, "echo"),
            (CatalogRowState::Canceled, "foxtrot"),
            (CatalogRowState::Done, "dir/gamma|gamma"),
        ];
        for (state, link) in cases {
            let row = render_row(link, state, Some("Some criterion"));
            let text = format!(
                "---\n---\n# Host\n\n## Projects\n\n> [!projects]\n>\n{row}\n"
            );
            let CatalogRead::Rows(rows) = parse_catalog(&text) else {
                panic!("row should parse: {row}");
            };
            assert_eq!(rows.len(), 1, "{row}");
            let expected_key = link
                .rsplit('|')
                .next()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_ascii_lowercase();
            assert_eq!(rows[0].key, expected_key, "{row}");
            assert_eq!(rows[0].state, state, "{row}");
        }
    }

    #[test]
    fn criterion_cleaning_covers_real_shapes() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("#task Ship it #hide", "ship", Some("Ship it")),
            (
                "#task [sase](https://sase.sh) launch #prj",
                "sase",
                Some("sase launch"),
            ),
            (
                "#task Call dentist [💡](https://keep.google.com/u/0/#NOTE/id \"Open in Google Keep\") [created:: 2026-09-27]",
                "dentist",
                Some("Call dentist"),
            ),
            (
                "#task Write [fresh:: 2026-01-01] [keeps:: 3] [dependsOn:: x] docs",
                "docs",
                Some("Write docs"),
            ),
            (
                "#task Ship 📅 2026-12-01 now",
                "ship",
                Some("Ship now"),
            ),
            (
                "#task Fix [[beta|Beta UI]] bug",
                "fix",
                Some("Fix Beta UI bug"),
            ),
            (
                "#task Fix [[dir/gamma]] bug",
                "fix",
                Some("Fix gamma bug"),
            ),
            (
                "#task <short_project_completion_criteria_goes_here>",
                "ship",
                None,
            ),
            ("#task Alpha", "alpha", None),
            ("#task #prj", "empty", None),
        ];
        for (raw, stem, expected) in cases {
            assert_eq!(
                clean_criterion(raw, stem).as_deref(),
                *expected,
                "{raw}"
            );
        }
    }

    #[test]
    fn open_rows_sort_by_stem_and_closed_newest_first() {
        let host = Path::new("host.md");
        let rows = vec![
            child("bravo", CatalogRowState::Blocked, None),
            child("alpha", CatalogRowState::Scheduled, None),
            child("Charlie", CatalogRowState::Open, None),
            child("old", CatalogRowState::Done, Some("2026-11-01")),
            child("new", CatalogRowState::Canceled, Some("2026-11-12")),
            child("undated", CatalogRowState::Done, None),
        ];
        let plan =
            plan_host_catalog(host, "host", HostKind::Project, "# H\n", rows);
        let stems: Vec<&str> =
            plan.rows.iter().map(|row| row.stem.as_str()).collect();
        assert_eq!(
            stems,
            vec!["alpha", "bravo", "Charlie", "new", "old", "undated"],
            "open A-Z, then newest-first with undated last"
        );
        assert_eq!(
            (plan.open, plan.blocked, plan.warning, plan.closed),
            (1, 2, 0, 3)
        );
        let after = plan.edit.expect("host edit").after;
        let closed_at = after.find("> - **Closed**").expect("group row");
        let first_open = after.find("[[alpha]]").expect("open row");
        assert!(first_open < closed_at, "{after}");
    }

    #[test]
    fn membership_follows_exact_basename_and_exclusion_rules() {
        let map = notes(&[
            ("parent.md", &project_note("wip", None, "- [ ] #task P ^prj")),
            ("docs/other.md", &project_note("wip", None, "- [ ] #task O ^prj")),
            (
                "sub/kid.md",
                &project_note("wip", Some("docs/other"), "- [ ] #task E ^prj"),
            ),
            ("kid.md", &project_note("wip", None, "- [ ] #task K ^prj")),
            ("base.md", &project_note("wip", Some("parent"), "- [ ] #task B ^prj")),
            ("selfish.md", &project_note("wip", Some("selfish"), "- [ ] #task S ^prj")),
            (
                "listed.md",
                "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[a]], [[b]]\"\n---\n- [ ] #task L ^prj\n",
            ),
            ("done/hidden.md", &project_note("wip", Some("parent"), "- [ ] #task H ^prj")),
            ("a/dup.md", &project_note("wip", None, "- [ ] #task A ^prj")),
            ("b/dup.md", &project_note("wip", None, "- [ ] #task B ^prj")),
            ("twin.md", &project_note("wip", Some("dup"), "- [ ] #task T ^prj")),
        ]);
        let plan = plan_catalogs(&map, &index_of(&map), TODAY);
        let parent = plan.host(Path::new("parent.md")).expect("parent host");
        assert_eq!(
            parent
                .rows
                .iter()
                .map(|row| row.stem.as_str())
                .collect::<Vec<_>>(),
            vec!["base"],
            "basename match lists; done/, self, and list-form parents do not"
        );
        // Exact vault-relative targets keep working, with a path link when
        // the stem alone would resolve to a different note.
        let other = plan.host(Path::new("docs/other.md")).expect("other host");
        assert_eq!(other.rows.len(), 1);
        assert_eq!(other.rows[0].stem, "kid");
        assert_eq!(other.rows[0].link, "sub/kid|kid");
        // Ambiguous basenames resolve to nothing.
        assert!(plan.host(Path::new("a/dup.md")).is_none());
        assert!(plan.host(Path::new("b/dup.md")).is_none());
        assert!(plan.host(Path::new("selfish.md")).is_none());
    }

    #[test]
    fn area_and_terminal_hosts_plan_but_empty_hosts_do_not() {
        let map = notes(&[
            ("area.md", &area_note()),
            (
                "kid.md",
                &project_note("wip", Some("area"), "- [ ] #task K ^prj"),
            ),
            (
                "old.md",
                &project_note(
                    "done",
                    None,
                    "- [x] #task O [completion:: 2026-11-10] ^prj",
                ),
            ),
            (
                "grandkid.md",
                &project_note(
                    "done",
                    Some("old"),
                    "- [x] #task G [completion:: 2026-11-11] ^prj",
                ),
            ),
            (
                "lonely.md",
                &project_note("wip", None, "- [ ] #task L ^prj"),
            ),
        ]);
        let plan = plan_catalogs(&map, &index_of(&map), TODAY);
        let area = plan.host(Path::new("area.md")).expect("area host");
        assert_eq!(area.kind, HostKind::Area);
        assert_eq!(area.rows.len(), 1);
        let old = plan.host(Path::new("old.md")).expect("terminal host");
        assert_eq!(old.kind, HostKind::Project);
        assert_eq!(old.closed, 1);
        assert!(plan.host(Path::new("lonely.md")).is_none());
    }

    #[test]
    fn plan_records_carry_kind_counts_edits_and_child_detail() {
        let map = notes(&[
            (
                "parent.md",
                "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Parent ^prj\n",
            ),
            (
                "kid.md",
                &project_note(
                    "done",
                    Some("parent"),
                    "- [x] #task Kid work [completion:: 2026-11-10] ^prj",
                ),
            ),
        ]);
        let plan = plan_catalogs(&map, &index_of(&map), TODAY);
        let host = plan.host(Path::new("parent.md")).expect("parent");
        assert_eq!(host.stem, "parent");
        assert_eq!(host.kind.as_str(), "project");
        assert_eq!(host.state.as_str(), "missing");
        assert_eq!(plan.edits().len(), 1);
        let row = &host.rows[0];
        assert!(!row.is_open());
        assert_eq!(row.prj_line, 6);
        assert_eq!(row.prj_mark, 'x');
        assert!(!row.prj_hidden);
        assert!(!row.prj_blocked_tag);
        assert_eq!(row.closed_on_raw.as_deref(), Some("2026-11-10"));
        assert_eq!(host.issues.len(), 1);
        assert_eq!(host.issues[0].code, "projects_catalog_stale");
    }

    #[test]
    fn empty_host_removes_a_stale_callout_but_keeps_prose() {
        let contents = "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task P ^prj\n\n## Projects\n\nAuthored prose.\n\n> [!projects]\n>\n> - **[[gone]]** — Old.\n";
        let plan = plan_host_catalog(
            Path::new("host.md"),
            "host",
            HostKind::Project,
            contents,
            Vec::new(),
        );
        assert_eq!(plan.state, CatalogState::Stale);
        let after = plan.edit.expect("removal edit").after;
        assert!(!after.contains("[!projects]"), "{after}");
        assert!(after.contains("## Projects"), "{after}");
        assert!(after.contains("Authored prose."), "{after}");
    }

    #[test]
    fn legacy_marker_and_callout_install_in_one_postimage() {
        let map = notes(&[
            (
                "parent.md",
                "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Parent ^prj\n\t- 🧩 **Sub-projects:** [[kid]]\n",
            ),
            ("kid.md", &project_note("wip", Some("parent"), "- [ ] #task Kid ^prj")),
        ]);
        let plan = plan_catalogs(&map, &index_of(&map), TODAY);
        let parent = plan.host(Path::new("parent.md")).expect("parent");
        let after = parent.edit.as_ref().expect("edit").after.clone();
        assert!(!after.contains("Sub-projects"), "{after}");
        assert!(after.contains("> [!projects]"), "{after}");
        assert!(after.contains("[[kid]]"), "{after}");
    }

    #[test]
    fn legacy_removal_needs_exactly_one_prj() {
        let one = "- [ ] #task P ^prj\n\t- 🧩 **Sub-projects:** [[a]]\n\t\t- subtree\n";
        let removed = remove_legacy_subprojects_lines(one);
        assert!(!removed.contains("Sub-projects"), "{removed}");
        assert!(removed.contains("- [ ] #task P ^prj"), "{removed}");
        let none = "- [ ] #task P\n\t- 🧩 **Sub-projects:** [[a]]\n";
        assert_eq!(remove_legacy_subprojects_lines(none), none);
        let two = "- [ ] #task A ^prj\n\t- 🧩 **Sub-projects:** [[a]]\n- [ ] #task B ^prj\n";
        assert_eq!(remove_legacy_subprojects_lines(two), two);
    }

    #[test]
    fn crlf_bom_and_missing_eof_newline_survive() {
        let host = Path::new("host.md");
        let rows = vec![child("alpha", CatalogRowState::Open, None)];
        let crlf = "---\r\ntype: \"[[project]]\"\r\nstatus: wip\r\n---\r\n- [ ] #task P ^prj\r\n";
        let after =
            plan_host_catalog(host, "host", HostKind::Project, crlf, rows)
                .edit
                .expect("edit")
                .after;
        assert!(!after.replace("\r\n", "").contains('\n'), "{after:?}");
        let bom = "\u{feff}---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task P ^prj\n";
        let after = plan_host_catalog(
            host,
            "host",
            HostKind::Project,
            bom,
            vec![child("alpha", CatalogRowState::Open, None)],
        )
        .edit
        .expect("edit")
        .after;
        assert!(after.starts_with("\u{feff}"), "{after:?}");
        // No-final-newline inputs stay intact through legacy removal.
        let noeol = "- [ ] #task P ^prj\n\t- 🧩 **Sub-projects:** [[a]]";
        assert_eq!(
            remove_legacy_subprojects_lines(noeol),
            "- [ ] #task P ^prj\n"
        );
    }

    #[test]
    fn damaged_hosts_keep_their_text_and_report_the_line() {
        let contents = "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task P ^prj\n\n## Projects\n\n> [!projects\n> - partial\n";
        let plan = plan_host_catalog(
            Path::new("host.md"),
            "host",
            HostKind::Project,
            contents,
            vec![child("alpha", CatalogRowState::Open, None)],
        );
        assert_eq!(plan.state, CatalogState::Damaged);
        assert_eq!(plan.state.as_str(), "damaged");
        assert!(plan.edit.is_none());
        assert_eq!(plan.issues[0].code, "projects_catalog_damaged");
        assert_eq!(plan.issues[0].line, Some(9));
        assert!(plan.issues[0].message.contains("host.md"));
    }

    #[test]
    fn golden_host_with_forty_children_round_trips() {
        let mut pairs: Vec<(String, String)> = vec![(
            "sase.md".to_string(),
            "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Sase ^prj\n"
                .to_string(),
        )];
        for index in 0..44 {
            let stem = format!("p{index:02}");
            let day = (index % 27) + 1;
            let (status, prj) = match index % 6 {
                0 => ("wip", format!("- [ ] #task Criterion {stem} ^prj")),
                1 => (
                    "wip",
                    format!(
                        "- [ ] #task Criterion {stem} [scheduled:: 2026-12-01] ^prj"
                    ),
                ),
                2 => ("wip", format!("- [?] #task Criterion {stem} ^prj")),
                3 => ("wip", "- [ ] #task No lifecycle here".to_string()),
                4 => (
                    "done",
                    format!(
                        "- [x] #task Criterion {stem} [completion:: 2026-11-{day:02}] ^prj"
                    ),
                ),
                _ => (
                    "canceled",
                    format!(
                        "- [-] #task Criterion {stem} [cancelled:: 2026-10-{day:02}] ^prj"
                    ),
                ),
            };
            pairs.push((
                format!("{stem}.md"),
                project_note(status, Some("sase"), &prj),
            ));
        }
        let map: BTreeMap<PathBuf, String> = pairs
            .into_iter()
            .map(|(path, contents)| (PathBuf::from(path), contents))
            .collect();
        let plan = plan_catalogs(&map, &index_of(&map), TODAY);
        let host = plan.host(Path::new("sase.md")).expect("sase host");
        assert_eq!(host.rows.len(), 44);
        assert_eq!(
            (host.open, host.blocked, host.warning, host.closed),
            (8, 15, 7, 14),
            "ready, scheduled+blocked, warning, and closed tallies"
        );
        let after = host.edit.as_ref().expect("host edit").after.clone();
        assert!(after.contains("> - **Closed**"), "{after}");
        // Every planned row survives the write and parses back identically.
        let CatalogRead::Rows(parsed) = parse_catalog(&after) else {
            panic!("golden callout should parse");
        };
        assert_eq!(parsed.len(), 44);
        for (planned, actual) in host.rows.iter().zip(parsed.iter()) {
            let expected_key = planned
                .link
                .rsplit('|')
                .next()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_ascii_lowercase();
            assert_eq!(actual.key, expected_key, "{}", planned.stem);
            assert_eq!(actual.state, planned.state, "{}", planned.stem);
        }
        // Open rows stay A-Z across glyph flips; closed rows are newest
        // first with undated last (warnings have no dates here, only the
        // malformed child is open-dated... assert the documented order).
        let open: Vec<&str> = host
            .rows
            .iter()
            .filter(|row| row.state.is_open())
            .map(|row| row.stem.as_str())
            .collect();
        let mut sorted = open.clone();
        sorted.sort_by_key(|stem| stem.to_lowercase());
        assert_eq!(open, sorted);
        // children_by_host mirrors the plan for sync's #hide rule.
        let by_host = plan.children_by_host();
        assert_eq!(by_host[Path::new("sase.md")].len(), 44);
    }
}
