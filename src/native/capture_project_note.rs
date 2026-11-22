//! Pure project-note content renderer for the `+` sigil capture family.
//!
//! `bob capture '@cash^goog-exit+' '...'` creates a brand-new sub-project note
//! `<route>_<block_id>.md` whose `parent` frontmatter points back at
//! `<route>.md`, mirroring the Obsidian Bob Navigation Hotkeys command
//! "Create project note from task". This module renders that note's basename
//! and contents from an already-parsed capture item.
//!
//! The module is free of filesystem and clock access: the caller passes the
//! resolved body, the current datetime (`bob_env::current_datetime()`), the
//! resolved scheduled date, the resolved priority field, pre-rendered
//! schedule-log lines, the item's authored sub-bullets with their task
//! IDs, and the note-task settings used to canonicalize requirement rows.
//! `render_project_note` returns the file contents plus a small
//! summary (`basename`, rendered `^prj` line, section titles, task count) for
//! the execution layer's JSON contract.
//!
//! Section mapping follows `buildProjectSeedFromChildBullets` in
//! `plugins/bob-navigation-hotkeys/main.js`: a first-level authored bullet is
//! a section when it has at least one nested bullet and its body matches the
//! ALL-CAPS title shape, otherwise it is a task. Nested checkbox bullets
//! under a task row are that row's requirements: the row is canonicalized
//! so reqs sit in the reqs zone and loose notes file into `NOTES` (created
//! only when a note lands). Rows without reqs, sections, moves, and the
//! `^prj` line stay as authored. Managed schedule/work log markers are
//! deliberately not special-cased here: a capture draft authors a brand-new
//! task rather than moving an existing one, so a log-shaped bullet is an
//! ordinary task or section bullet.

use std::collections::{BTreeMap, HashMap};

use chrono::{
    Local, LocalResult, NaiveDate, NaiveDateTime, Offset as _, TimeZone,
};
use chrono_tz::Tz;

use super::capture_dependency_tasks::has_hide_tag;
use super::capture_language::{split_leading_checkbox, AuthoredSubBullet};
use super::note_tasks::{NoteTaskSettings, TaskStatusType};
use super::projects::edits::normalize_prj_is_blocked_tag;
use super::task_block_layout::{canonicalize_new_req_block, CanonicalReqBlock};

/// Placeholder task kept under `## Tasks` when no authored task children were
/// rendered. Matches the template's `(REPLACE WITH TASK DESCRIPTION)` line
/// that `replaceProjectTasksPlaceholder` leaves in place in that case.
pub(crate) const PLACEHOLDER_TASK_BODY: &str =
    "(REPLACE WITH TASK DESCRIPTION)";

/// Everything `render_project_note` needs. Dates arrive resolved: `scheduled`
/// is `YYYY-MM-DD`, `priority` is the `(name, value)` field pair (for example
/// `("priority", "lowest")`), and `schedule_log_lines` are pre-rendered,
/// already-indented lines (see `capture_schedule_log::plan`) inserted verbatim
/// under the `^prj` task. The `^prj` task is never linked: it renders `[ ]`,
/// or `[?]` when scheduled.
#[derive(Debug, Clone)]
pub(crate) struct ProjectNoteRenderInput<'a> {
    pub(crate) route: &'a str,
    pub(crate) block_id: &'a str,
    pub(crate) body: &'a str,
    pub(crate) now: NaiveDateTime,
    /// Bob's configured zone for the `created` stamp offset. `now` is
    /// already wall-clock time in this zone; `None` resolves via the host.
    pub(crate) timezone: Option<Tz>,
    pub(crate) scheduled: Option<&'a str>,
    pub(crate) priority: Option<(&'a str, &'a str)>,
    pub(crate) schedule_log_lines: &'a [String],
    pub(crate) sub_bullets: &'a [AuthoredSubBullet],
    pub(crate) status_types: BTreeMap<char, TaskStatusType>,
    /// Settings for canonicalizing a `## Tasks` row that owns requirements.
    /// The renderer never reads the vault; the caller supplies them.
    pub(crate) settings: &'a NoteTaskSettings,
}

/// Rendered note plus the summary the execution layer reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedProjectNote {
    /// File basename, for example `cash_goog_exit.md`.
    pub(crate) basename: String,
    /// Full file contents.
    pub(crate) contents: String,
    /// The rendered `^prj` lifecycle task line.
    pub(crate) task_line: String,
    /// Rendered section titles in source order, excluding `## Tasks`.
    pub(crate) sections: Vec<String>,
    /// Number of top-level task lines under `## Tasks`, including the
    /// placeholder when it is kept. Nested child lines and notes merged from
    /// an authored `TASKS` section are not counted.
    pub(crate) task_count: usize,
    /// Named first-level tasks in source order, backing `task_links`.
    pub(crate) named_tasks: Vec<RenderedNamedTask>,
    /// Every rendered `## Tasks` row (not section headings or note-only
    /// bullets), in output order, with source identity for offer selection.
    pub(crate) task_rows: Vec<RenderedTask>,
    /// Final zero-based task-root slots for authored move bullets. Slots
    /// count task roots only; headings and section-note bullets do not count.
    pub(crate) move_slots: Vec<ProjectNoteMoveSlot>,
    /// One entry per `## Tasks` row that owns requirements, in output order.
    pub(crate) req_writes: Vec<CanonicalReqBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectNoteMoveSlot {
    pub(crate) source_index: usize,
    pub(crate) slot: usize,
}

/// One named first-level task bullet: a trailing ` :id` / ` ^id` token
/// rendered as a task line ending in ` ^<id>`. `link` is `true` for `:`
/// (Next and linked into the Pomodoro) and `false` for `^` (named only).
/// `text` is the task body without a leading checkbox, and `task_line` is
/// the full rendered line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedNamedTask {
    pub(crate) block_id: String,
    pub(crate) link: bool,
    pub(crate) text: String,
    pub(crate) task_line: String,
}

/// One rendered `## Tasks` row, including unnamed, hidden, closed, and
/// placeholder tasks. Nested context lines and note-only bullets are
/// omitted. `source_index` is the authored first-level sub-bullet that
/// produced the row, so Offer selection can flip that bullet to ` :`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedTask {
    pub(crate) line: usize,
    pub(crate) status: char,
    pub(crate) block_id: Option<String>,
    pub(crate) text: String,
    pub(crate) link: bool,
    pub(crate) hidden: bool,
    pub(crate) placeholder: bool,
    pub(crate) source_index: Option<usize>,
    pub(crate) task_line: String,
}

/// `<route>_<block-id with every '-' replaced by '_'>.md`, matching
/// `getProjectBasenameFromTaskBlockId`. The route arrives lower-cased from the
/// token parser; the block ID keeps its authored case.
pub(crate) fn project_note_basename(route: &str, block_id: &str) -> String {
    format!("{}_{}.md", route, block_id.replace('-', "_"))
}

/// Format the frontmatter `created` stamp (`YYYY-MM-DDTHH:mm:ss±ZZZZ`) from a
/// naive datetime that is already wall-clock time in Bob's configured zone.
/// The offset is resolved for that naive value via the passed zone rather
/// than calling `Local::now()` again, so `BOB_NOW` keeps controlling the
/// field and UTC-clock hosts still stamp the configured offset. `None` keeps
/// the legacy host-local (`chrono::Local`) resolution.
pub(crate) fn format_created_timestamp(
    now: NaiveDateTime,
    timezone: Option<Tz>,
) -> String {
    const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%z";
    format_timestamp_with_format(now, timezone, FORMAT)
}

/// Format the minute-precision `captured` stamp (`YYYY-MM-DDTHH:mm±ZZZZ`)
/// from the same naive local datetime, with the same offset resolution as
/// `format_created_timestamp` so `created` and `captured` always agree on
/// offsets (including ambiguous times and DST gaps).
pub(crate) fn format_captured_timestamp(
    now: NaiveDateTime,
    timezone: Option<Tz>,
) -> String {
    const FORMAT: &str = "%Y-%m-%dT%H:%M%z";
    format_timestamp_with_format(now, timezone, FORMAT)
}

/// Shared offset resolution for a caller-supplied `strftime` format.
pub(crate) fn format_timestamp_with_format(
    now: NaiveDateTime,
    timezone: Option<Tz>,
    format: &str,
) -> String {
    match timezone {
        Some(timezone) => format_in_zone(timezone, now, format),
        None => format_in_zone(Local, now, format),
    }
}

/// Resolve one naive wall time in `zone`, covering ambiguous times and the
/// spring-forward gap.
fn format_in_zone<Zone>(zone: Zone, now: NaiveDateTime, format: &str) -> String
where
    Zone: TimeZone,
    Zone::Offset: std::fmt::Display,
{
    match zone.from_local_datetime(&now) {
        LocalResult::Single(datetime) => datetime.format(format).to_string(),
        LocalResult::Ambiguous(earliest, _) => {
            earliest.format(format).to_string()
        }
        LocalResult::None => gap_offset_timestamp(zone, now, format),
    }
}

/// Fallback for nonexistent wall times in the spring-forward gap: borrow the
/// offset in effect an hour later so the authored wall time is preserved.
fn gap_offset_timestamp<Zone>(
    zone: Zone,
    now: NaiveDateTime,
    format: &str,
) -> String
where
    Zone: TimeZone,
    Zone::Offset: std::fmt::Display,
{
    const FALLBACK_SUFFIX: &str = "+0000";
    let later = now
        .checked_add_signed(chrono::Duration::hours(1))
        .unwrap_or(now);
    let offset = match zone.from_local_datetime(&later) {
        LocalResult::Single(datetime) => Some(datetime.offset().fix()),
        LocalResult::Ambiguous(earliest, _) => Some(earliest.offset().fix()),
        LocalResult::None => None,
    };
    offset
        .and_then(|offset| offset.from_local_datetime(&now).single())
        .map(|resolved| resolved.format(format).to_string())
        .unwrap_or_else(|| {
            let base_format = if format.contains("%S") {
                "%Y-%m-%dT%H:%M:%S"
            } else {
                "%Y-%m-%dT%H:%M"
            };
            format!("{}{FALLBACK_SUFFIX}", now.format(base_format))
        })
}

/// Whether a trimmed authored body is a project-note section title.
///
/// Delegates to [`crate::native::capture_drawers::is_drawer_title`] so the
/// two grammars never drift: drawer sentence punctuation (`?`, `!`, `;`,
/// trailing `:`) qualifies here too.
pub(crate) fn is_project_section_title(body: &str) -> bool {
    crate::native::capture_drawers::is_drawer_title(body.trim())
}

/// Lowercase the whole body, then uppercase the first character of every
/// alphanumeric run: `FUTURE WORK` becomes `Future Work`, `NON-GOALS` becomes
/// `Non-Goals`, and acronyms are not preserved, so `API DESIGN` becomes
/// `Api Design`. Mirrors `formatProjectSectionTitle`.
pub(crate) fn format_project_section_title(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = collapsed.to_lowercase();
    let mut rendered = String::with_capacity(lower.len());
    let mut run_start = true;
    for character in lower.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if run_start {
                rendered.push(character.to_ascii_uppercase());
                run_start = false;
            } else {
                rendered.push(character);
            }
        } else {
            rendered.push(character);
            run_start = true;
        }
    }
    rendered
}

/// Trimmed, whitespace-collapsed, casefolded comparison key. Two section
/// bullets with equal keys merge into one section; a rendered title equal to
/// an existing `##` header's key appends to that header instead of adding a
/// second one. Mirrors `normalizeProjectSectionTitle`.
pub(crate) fn normalize_project_section_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `body` already carries a `#task` token, using the same boundary
/// rule as `PROJECT_TASK_TAG_RE`: `(^|[\s([{])#task($|[\s)\]},.;:!?])`.
fn contains_task_tag(body: &str) -> bool {
    let bytes = body.as_bytes();
    let mut index = 0;
    while let Some(found) = body[index..].find("#task") {
        let start = index + found;
        let before_ok = start == 0
            || matches!(
                bytes[start - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'(' | b'[' | b'{'
            );
        let end = start + "#task".len();
        let after_ok = end == bytes.len()
            || matches!(
                bytes[end],
                b' ' | b'\t'
                    | b'\n'
                    | b'\r'
                    | b')'
                    | b']'
                    | b'}'
                    | b','
                    | b'.'
                    | b';'
                    | b':'
                    | b'!'
                    | b'?'
            );
        if before_ok && after_ok {
            return true;
        }
        index = start + 1;
    }
    false
}

/// One authored bullet rendered beneath a line at `base_level`: a tab per
/// level below it, so `\t- note` sits under a task row and a deeper
/// subtree keeps its shape.
fn nested_line(bullet: &AuthoredSubBullet, base_level: u8) -> String {
    let tabs = bullet.depth.level().saturating_sub(base_level);
    format!("{}- {}", "\t".repeat(usize::from(tabs)), bullet.body.trim())
}

/// Canonicalize one `## Tasks` row when it owns requirements: reqs sit in
/// the reqs zone and loose notes file into `NOTES`. A row with no reqs
/// stays byte-identical.
fn canonicalize_task_row(
    task_line: String,
    nested: &[&AuthoredSubBullet],
    owner_level: u8,
    settings: &NoteTaskSettings,
    req_writes: &mut Vec<CanonicalReqBlock>,
) -> Vec<String> {
    let mut row_lines = Vec::with_capacity(1 + nested.len());
    row_lines.push(task_line);
    for bullet in nested {
        row_lines.push(nested_line(bullet, owner_level));
    }
    let row_block = row_lines.join("\n");
    match canonicalize_new_req_block(&row_block, settings) {
        Some(canonical) => {
            let lines = canonical.updated.lines().map(str::to_string).collect();
            req_writes.push(canonical);
            lines
        }
        None => row_lines,
    }
}

/// Render one authored task line: preserve an authored checkbox status
/// (defaulting to open), add `#task` unless present, append
/// `[created::DATE]` unless the body already carries one, and finally append
/// ` ^<block-id>` for a named task. Nested child lines are rendered by the
/// caller as plain indented bullets.
fn render_authored_task(
    status: char,
    body: &str,
    created_date: &str,
    task_id: Option<&str>,
) -> String {
    let task_body = if body.is_empty() {
        "#task".to_string()
    } else if contains_task_tag(body) {
        body.to_string()
    } else {
        format!("#task {body}")
    };
    let mut line = format!("- [{status}] {task_body}");
    if !task_body.contains("[created::") {
        line.push_str(&format!(" [created::{created_date}]"));
    }
    if let Some(id) = task_id {
        line.push_str(&format!(" ^{id}"));
    }
    line
}

struct FirstGroup<'a> {
    owner: &'a AuthoredSubBullet,
    owner_index: usize,
    nested: Vec<&'a AuthoredSubBullet>,
}

struct PendingTask {
    status: char,
    block_id: Option<String>,
    text: String,
    link: bool,
    hidden: bool,
    placeholder: bool,
    source_index: Option<usize>,
    task_line: String,
}

struct SectionEntry {
    title: String,
    normalized: String,
    note_lines: Vec<String>,
}

/// Render the project note for `input`. See the module docs for the contract.
pub(crate) fn render_project_note(
    input: &ProjectNoteRenderInput<'_>,
) -> RenderedProjectNote {
    let basename = project_note_basename(input.route, input.block_id);
    let created_date = input.now.format("%Y-%m-%d").to_string();
    let created_timestamp = format_created_timestamp(input.now, input.timezone);

    let status = if input.scheduled.is_some() { '?' } else { ' ' };
    let mut task_line =
        format!("- [{status}] #task #prj {}", input.body.trim());
    if let Some((name, value)) = input.priority {
        task_line.push_str(&format!(" [{name}::{value}]"));
    }
    if let Some(scheduled) = input.scheduled {
        task_line.push_str(&format!(" [scheduled:: {scheduled}]"));
    }
    task_line.push_str(" #hide ^prj");

    let mut groups: Vec<FirstGroup<'_>> = Vec::new();
    for (index, bullet) in input.sub_bullets.iter().enumerate() {
        if bullet.depth.is_first() || groups.is_empty() {
            groups.push(FirstGroup {
                owner: bullet,
                owner_index: index,
                nested: Vec::new(),
            });
        } else if let Some(current) = groups.last_mut() {
            current.nested.push(bullet);
        }
    }

    // A future project date blocks the whole note (§13.3, PDG5): every
    // open resident lands `[?]` plus the machine-owned tag. Only `:`
    // tasks also request a Task Link, which the planner leaves unlinked.
    let project_blocked = input.scheduled.is_some_and(|date| {
        NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .is_ok_and(|parsed| parsed > input.now.date())
    });

    let mut tasks_block: Vec<String> = Vec::new();
    let mut task_entries: usize = 0;
    let mut pending_tasks: Vec<PendingTask> = Vec::new();
    let mut req_writes: Vec<CanonicalReqBlock> = Vec::new();
    let mut move_slots = Vec::new();
    let mut sections: Vec<SectionEntry> = Vec::new();
    let mut section_index: HashMap<String, usize> = HashMap::new();
    for group in &groups {
        if group.owner.move_task.is_some() {
            move_slots.push(ProjectNoteMoveSlot {
                source_index: group.owner_index,
                slot: task_entries + move_slots.len(),
            });
            continue;
        }
        if group.owner.move_choice.is_some() {
            // `- ^^` contributes nothing until `cli-execute` splices picks.
            continue;
        }
        let owner_body = group.owner.body.trim();
        let (checkbox, bare) = split_leading_checkbox(owner_body);
        let named = group.owner.task_id.as_ref();
        if named.is_none()
            && checkbox.is_none()
            && !group.nested.is_empty()
            && is_project_section_title(owner_body)
        {
            let title = format_project_section_title(owner_body);
            let normalized = normalize_project_section_title(&title);
            // Section entries sit at the section's left edge; anything
            // deeper keeps its authored shape beneath its entry.
            let note_lines: Vec<String> = group
                .nested
                .iter()
                .map(|nested| {
                    nested_line(nested, group.owner.depth.level() + 1)
                })
                .collect();
            match section_index.get(&normalized) {
                Some(&index) => {
                    sections[index].note_lines.extend(note_lines);
                }
                None => {
                    section_index.insert(normalized.clone(), sections.len());
                    sections.push(SectionEntry {
                        title,
                        normalized,
                        note_lines,
                    });
                }
            }
        } else {
            let authored_status = match named {
                Some(task) if task.link => {
                    if input.scheduled.is_some() {
                        '?'
                    } else {
                        '*'
                    }
                }
                _ => checkbox.unwrap_or(' '),
            };
            let open = input
                .status_types
                .get(&authored_status)
                .is_some_and(|status_type| status_type.is_open());
            let task_status = if project_blocked && open {
                '?'
            } else {
                authored_status
            };
            let task_id_suffix = named.map(|task| task.block_id.as_str());
            let task_line = render_authored_task(
                task_status,
                bare,
                &created_date,
                task_id_suffix,
            );
            let task_line = if project_blocked && open {
                normalize_prj_is_blocked_tag(&task_line, true)
            } else {
                task_line
            };
            let link = named.is_some_and(|task| task.link);
            let hidden = has_hide_tag(&task_line) || has_hide_tag(bare);
            pending_tasks.push(PendingTask {
                status: task_status,
                block_id: named.map(|task| task.block_id.clone()),
                text: bare.to_string(),
                link,
                hidden,
                placeholder: false,
                source_index: Some(group.owner_index),
                task_line: task_line.clone(),
            });
            let row_lines = canonicalize_task_row(
                task_line,
                &group.nested,
                group.owner.depth.level(),
                input.settings,
                &mut req_writes,
            );
            tasks_block.extend(row_lines);
            task_entries += 1;
        }
    }

    // A rendered section matching the generated `## Tasks` header merges into
    // it instead of adding a duplicate header. The fresh note holds no other
    // headers, so this covers the general match-an-existing-header rule.
    let mut merged_tasks: Vec<String> = Vec::new();
    sections.retain(|section| {
        if section.normalized == "tasks" {
            merged_tasks.extend(section.note_lines.iter().cloned());
            false
        } else {
            true
        }
    });

    let mut contents = String::new();
    contents.push_str("---\n");
    contents.push_str(&format!("parent: \"[[{}]]\"\n", input.route));
    contents.push_str("template: \"[[new_project]]\"\n");
    contents.push_str("type: \"[[project]]\"\n");
    contents.push_str("status: wip\n");
    contents.push_str(&format!("created: {created_timestamp}\n"));
    contents.push_str("---\n\n");
    contents.push_str(&task_line);
    contents.push('\n');
    for line in input.schedule_log_lines {
        contents.push_str(line);
        contents.push('\n');
    }
    contents.push_str("\n## Tasks\n\n");
    let task_count = if tasks_block.is_empty()
        && merged_tasks.is_empty()
        && move_slots.is_empty()
    {
        let placeholder_line = format!(
            "- [ ] #task {PLACEHOLDER_TASK_BODY} [created::{created_date}]"
        );
        pending_tasks.push(PendingTask {
            status: ' ',
            block_id: None,
            text: PLACEHOLDER_TASK_BODY.to_string(),
            link: false,
            hidden: false,
            placeholder: true,
            source_index: None,
            task_line: placeholder_line.clone(),
        });
        contents.push_str(&placeholder_line);
        contents.push('\n');
        1
    } else {
        for line in &tasks_block {
            contents.push_str(line);
            contents.push('\n');
        }
        for line in &merged_tasks {
            contents.push_str(line);
            contents.push('\n');
        }
        task_entries + move_slots.len()
    };

    let mut section_titles = Vec::with_capacity(sections.len());
    for section in &sections {
        section_titles.push(section.title.clone());
        contents.push_str(&format!("\n## {}\n\n", section.title));
        for line in &section.note_lines {
            contents.push_str(line);
            contents.push('\n');
        }
    }

    let named_tasks = pending_tasks
        .iter()
        .filter(|task| !task.placeholder)
        .filter_map(|task| {
            task.block_id.as_ref().map(|block_id| RenderedNamedTask {
                block_id: block_id.clone(),
                link: task.link,
                text: task.text.clone(),
                task_line: task.task_line.clone(),
            })
        })
        .collect();
    let mut task_rows: Vec<RenderedTask> = pending_tasks
        .into_iter()
        .map(|task| RenderedTask {
            line: 0,
            status: task.status,
            block_id: task.block_id,
            text: task.text,
            link: task.link,
            hidden: task.hidden,
            placeholder: task.placeholder,
            source_index: task.source_index,
            task_line: task.task_line,
        })
        .collect();
    assign_rendered_task_lines(&contents, &mut task_rows);

    RenderedProjectNote {
        basename,
        contents,
        task_line,
        sections: section_titles,
        task_count,
        named_tasks,
        task_rows,
        move_slots,
        req_writes,
    }
}

fn assign_rendered_task_lines(contents: &str, tasks: &mut [RenderedTask]) {
    let lines: Vec<&str> = contents.lines().collect();
    let mut start = 0;
    for task in tasks.iter_mut() {
        if let Some(offset) = lines[start..]
            .iter()
            .position(|line| *line == task.task_line)
        {
            let index = start + offset;
            task.line = index + 1;
            start = index + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::capture_language::{AuthoredDepth, ProjectTaskId};
    use super::super::note_tasks;
    use super::*;
    use chrono::NaiveDate;
    use std::path::Path;
    use std::sync::OnceLock;

    fn test_now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 20)
            .expect("valid test date")
            .and_hms_opt(14, 31, 7)
            .expect("valid test time")
    }

    fn bullet(body: &str, depth: AuthoredDepth) -> AuthoredSubBullet {
        AuthoredSubBullet {
            body: body.to_string(),
            depth,
            task_id: None,
            move_task: None,
            move_choice: None,
            drawer: None,
            req: None,
        }
    }

    fn named_bullet(
        body: &str,
        depth: AuthoredDepth,
        id: &str,
        link: bool,
    ) -> AuthoredSubBullet {
        AuthoredSubBullet {
            body: body.to_string(),
            depth,
            task_id: Some(ProjectTaskId {
                block_id: id.to_string(),
                link,
            }),
            move_task: None,
            move_choice: None,
            drawer: None,
            req: None,
        }
    }

    fn test_settings() -> &'static NoteTaskSettings {
        static SETTINGS: OnceLock<NoteTaskSettings> = OnceLock::new();
        SETTINGS.get_or_init(|| {
            note_tasks::read_settings(Path::new("/no-bob-vault"))
        })
    }

    fn base_input<'a>(
        sub_bullets: &'a [AuthoredSubBullet],
    ) -> ProjectNoteRenderInput<'a> {
        ProjectNoteRenderInput {
            route: "cash",
            block_id: "goog-exit",
            body: "Finish the Google exit packet!",
            now: test_now(),
            timezone: None,
            scheduled: None,
            priority: None,
            schedule_log_lines: &[],
            sub_bullets,
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
                ('X', TaskStatusType::Done),
                ('/', TaskStatusType::InProgress),
                ('*', TaskStatusType::OnHold),
                ('?', TaskStatusType::OnHold),
                ('-', TaskStatusType::Cancelled),
                ('>', TaskStatusType::Cancelled),
            ]),
            settings: test_settings(),
        }
    }

    fn pin_utc() {
        // Keep the rendered `created` offset deterministic. `TZ` is read
        // by libc, so it cannot be a thread-local override; every test
        // pins the same value exactly once per process instead of
        // racing on repeated process-global writes.
        crate::native::env::pin_tz_utc0_for_test();
    }

    #[test]
    fn basename_replaces_dashes_and_keeps_case() {
        assert_eq!(
            project_note_basename("cash", "goog-exit"),
            "cash_goog_exit.md"
        );
        assert_eq!(project_note_basename("sase", "TUI-Fix"), "sase_TUI_Fix.md");
    }

    #[test]
    fn basic_note_matches_the_plan_example() {
        pin_utc();
        let rendered = render_project_note(&base_input(&[]));
        assert_eq!(rendered.basename, "cash_goog_exit.md");
        assert_eq!(
            rendered.task_line,
            "- [ ] #task #prj Finish the Google exit packet! #hide ^prj"
        );
        assert_eq!(
            rendered.contents,
            "---\n\
             parent: \"[[cash]]\"\n\
             template: \"[[new_project]]\"\n\
             type: \"[[project]]\"\n\
             status: wip\n\
             created: 2026-09-20T14:31:07+0000\n\
             ---\n\
             \n\
             - [ ] #task #prj Finish the Google exit packet! #hide ^prj\n\
             \n\
             ## Tasks\n\
             \n\
             - [ ] #task (REPLACE WITH TASK DESCRIPTION) [created::2026-09-20]\n"
        );
        assert_eq!(rendered.sections, Vec::<String>::new());
        assert_eq!(rendered.task_count, 1);
        assert!(rendered.req_writes.is_empty());
    }

    #[test]
    fn created_timestamp_derives_from_the_passed_datetime() {
        pin_utc();
        let first = format_created_timestamp(test_now(), None);
        let later = format_created_timestamp(
            NaiveDate::from_ymd_opt(2026, 9, 21)
                .expect("valid test date")
                .and_hms_opt(9, 0, 0)
                .expect("valid test time"),
            None,
        );
        assert_eq!(first, "2026-09-20T14:31:07+0000");
        assert_eq!(later, "2026-09-21T09:00:00+0000");
    }

    #[test]
    fn configured_zone_controls_created_offset_on_utc_clock_hosts() {
        // Apollo runs Etc/UTC while Bob's day is America/New_York: `now`
        // is already New York wall time, and the configured zone must win
        // without consulting the host clock. This test pins nothing on
        // purpose; the `Some` branch never reads `Local`.
        let eastern: Tz = "America/New_York".parse().unwrap();
        let winter = NaiveDate::from_ymd_opt(2026, 11, 11)
            .expect("valid test date")
            .and_hms_opt(8, 1, 12)
            .expect("valid test time");
        assert_eq!(
            format_created_timestamp(winter, Some(eastern)),
            "2026-11-11T08:01:12-0500"
        );
        assert_eq!(
            format_captured_timestamp(winter, Some(eastern)),
            "2026-11-11T08:01-0500"
        );
        let summer = NaiveDate::from_ymd_opt(2026, 7, 11)
            .expect("valid test date")
            .and_hms_opt(8, 1, 12)
            .expect("valid test time");
        assert_eq!(
            format_created_timestamp(summer, Some(eastern)),
            "2026-07-11T08:01:12-0400"
        );
        assert_eq!(
            format_created_timestamp(winter, Some(chrono_tz::UTC)),
            "2026-11-11T08:01:12+0000"
        );
    }

    #[test]
    fn render_stamps_created_in_the_configured_zone() {
        // Simulate apollo's Etc/UTC clock: with the host zone at UTC the
        // old `Local` resolution stamped `+0000`, so only the configured
        // zone can produce `-0400` here.
        pin_utc();
        let mut input = base_input(&[]);
        input.timezone = Some("America/New_York".parse().unwrap());
        let rendered = render_project_note(&input);
        assert!(
            rendered
                .contents
                .contains("created: 2026-09-20T14:31:07-0400\n"),
            "{}",
            rendered.contents
        );
    }

    #[test]
    fn captured_timestamp_is_minute_precision_with_matching_offset() {
        pin_utc();
        let now = test_now();
        let captured = format_captured_timestamp(now, None);
        let created = format_created_timestamp(now, None);
        assert_eq!(captured, "2026-09-20T14:31+0000");
        // Same `Local` resolution, so `created` and `captured` agree on
        // offsets; the minute stamp is the created prefix without seconds.
        assert!(created.starts_with("2026-09-20T14:31:"));
        assert_eq!(&created[16..19], ":07");
        assert_eq!(&captured[16..], "+0000");
        assert_eq!(&created[19..], "+0000");
    }

    #[test]
    fn captured_timestamp_uses_shared_format_helper() {
        pin_utc();
        let now = test_now();
        assert_eq!(
            format_timestamp_with_format(now, None, "%Y-%m-%dT%H:%M%z"),
            format_captured_timestamp(now, None)
        );
        assert_eq!(
            format_timestamp_with_format(now, None, "%Y-%m-%dT%H:%M:%S%z"),
            format_created_timestamp(now, None)
        );
    }

    #[test]
    fn scheduled_date_lands_inline_on_prj_with_a_blocked_checkbox() {
        pin_utc();
        let mut input = base_input(&[]);
        input.scheduled = Some("2026-10-04");
        let rendered = render_project_note(&input);
        assert!(
            !rendered.contents.contains("\nscheduled:"),
            "{}",
            rendered.contents
        );
        assert_eq!(
            rendered.task_line,
            "- [?] #task #prj Finish the Google exit packet! \
             [scheduled:: 2026-10-04] #hide ^prj"
        );
        assert!(rendered.task_line.starts_with("- [?] "));
    }

    #[test]
    fn prj_task_is_never_linked() {
        let input = base_input(&[]);
        assert!(render_project_note(&input).task_line.starts_with("- [ ] "));
        let mut scheduled = base_input(&[]);
        scheduled.scheduled = Some("2026-10-04");
        assert!(render_project_note(&scheduled)
            .task_line
            .starts_with("- [?] "));
    }

    #[test]
    fn priority_writes_an_inline_field_before_hide() {
        let mut input = base_input(&[]);
        input.priority = Some(("priority", "lowest"));
        let rendered = render_project_note(&input);
        assert_eq!(
            rendered.task_line,
            "- [ ] #task #prj Finish the Google exit packet! \
             [priority::lowest] #hide ^prj"
        );
    }

    #[test]
    fn schedule_log_lines_land_directly_under_the_prj_task() {
        let log = vec![
            "\t- 🗓️ **SCHEDULE LOG**".to_string(),
            "\t\t- *2026-11-02* — 🎲 P0 → P4 · in **91** (91–365) days"
                .to_string(),
        ];
        let mut input = base_input(&[]);
        input.priority = Some(("priority", "lowest"));
        input.scheduled = Some("2026-11-02");
        input.schedule_log_lines = &log;
        let rendered = render_project_note(&input);
        let prj_index = rendered
            .contents
            .find(&rendered.task_line)
            .expect("prj line is rendered");
        let after = &rendered.contents[prj_index + rendered.task_line.len()..];
        assert!(after.starts_with("\n\t- 🗓️ **SCHEDULE LOG**\n"));
    }

    #[test]
    fn authored_tasks_render_with_created_stamps_and_tab_children() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::FIRST),
            bullet("Confirm the account number", AuthoredDepth::NESTED),
            bullet("Book the flight", AuthoredDepth::FIRST),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains(
            "- [ ] #task Call Morgan Stanley [created::2026-09-20]\n\
             \t- Confirm the account number\n\
             - [ ] #task Book the flight [created::2026-09-20]\n"
        ));
        assert_eq!(rendered.task_count, 2);
        assert!(!rendered.contents.contains(PLACEHOLDER_TASK_BODY));
    }

    fn deep(level: u8) -> AuthoredDepth {
        AuthoredDepth::from_level(level).expect("valid level")
    }

    #[test]
    fn deeper_bullets_under_a_task_row_keep_their_levels() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::FIRST),
            bullet("[ ] Confirm the account number", AuthoredDepth::NESTED),
            bullet("[ ] Find the statement", deep(3)),
            bullet("It is in the blue folder", deep(4)),
            bullet("Ask about fees", AuthoredDepth::NESTED),
            bullet("Book the flight", AuthoredDepth::FIRST),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains(
            "- [ ] #task Call Morgan Stanley [created::2026-09-20]\n\
             \t- [ ] Confirm the account number\n\
             \t\t- [ ] Find the statement\n\
             \t\t\t- It is in the blue folder\n\
             \t- NOTES\n\
             \t\t- Ask about fees\n\
             - [ ] #task Book the flight [created::2026-09-20]\n"
        ));
        assert_eq!(rendered.task_count, 2);
        assert_eq!(rendered.req_writes.len(), 1);
        assert_eq!(rendered.req_writes[0].owner, "Call Morgan Stanley");
        assert_eq!(rendered.req_writes[0].added, 2);
        assert_eq!(rendered.req_writes[0].notes_filed, 1);
        assert!(rendered.req_writes[0].notes_created);
    }

    #[test]
    fn deeper_bullets_under_a_section_entry_keep_their_levels() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::FIRST),
            bullet("FUTURE WORK", AuthoredDepth::FIRST),
            bullet("Consider the Roth option", AuthoredDepth::NESTED),
            bullet("Ask about the limits", deep(3)),
            bullet("Revisit the severance terms", AuthoredDepth::NESTED),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains(
            "- Consider the Roth option\n\
             \t- Ask about the limits\n\
             - Revisit the severance terms\n"
        ));
        assert_eq!(rendered.sections, vec!["Future Work".to_string()]);
    }

    #[test]
    fn all_caps_section_with_children_becomes_a_header() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::FIRST),
            bullet("FUTURE WORK", AuthoredDepth::FIRST),
            bullet("Consider the Roth option", AuthoredDepth::NESTED),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains("\n## Future Work\n\n"));
        assert!(rendered.contents.contains("- Consider the Roth option\n"));
        assert_eq!(rendered.sections, vec!["Future Work".to_string()]);
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn acronym_titles_do_not_preserve_capitals() {
        assert_eq!(format_project_section_title("API DESIGN"), "Api Design");
        assert_eq!(format_project_section_title("NON-GOALS"), "Non-Goals");
        assert_eq!(format_project_section_title("FUTURE WORK"), "Future Work");
    }

    #[test]
    fn equal_section_titles_merge_in_source_order() {
        let sub_bullets = vec![
            bullet("FUTURE WORK", AuthoredDepth::FIRST),
            bullet("First note", AuthoredDepth::NESTED),
            bullet("FUTURE  WORK", AuthoredDepth::FIRST),
            bullet("Second note", AuthoredDepth::NESTED),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(rendered.contents.matches("## Future Work").count(), 1);
        let section_start = rendered
            .contents
            .find("## Future Work")
            .expect("section is rendered");
        let section = &rendered.contents[section_start..];
        let first = section.find("- First note").expect("first note kept");
        let second = section.find("- Second note").expect("second note kept");
        assert!(first < second);
        assert_eq!(rendered.sections, vec!["Future Work".to_string()]);
    }

    #[test]
    fn bare_all_caps_bullet_without_children_stays_a_task() {
        let sub_bullets = vec![bullet("FUTURE WORK", AuthoredDepth::FIRST)];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(!rendered.contents.contains("## Future Work"));
        assert!(rendered
            .contents
            .contains("- [ ] #task FUTURE WORK [created::2026-09-20]\n"));
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn tasks_section_merges_into_the_generated_header() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::FIRST),
            bullet("TASKS", AuthoredDepth::FIRST),
            bullet("Carried reference note", AuthoredDepth::NESTED),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(rendered.contents.matches("## Tasks").count(), 1);
        assert!(rendered.contents.contains("- Carried reference note\n"));
        assert!(rendered.sections.is_empty());
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn section_title_shape_allows_sentence_punctuation() {
        assert!(is_project_section_title("FUTURE WORK"));
        assert!(is_project_section_title("Q&A"));
        assert!(is_project_section_title("WHAT'S NEXT?"));
        assert!(is_project_section_title("WHAT IS SASE?"));
        assert!(is_project_section_title("OPTIONS:"));
        assert!(!is_project_section_title("Future Work"));
        assert!(!is_project_section_title("NOTE: X"));
        assert!(!is_project_section_title("KEY:: VALUE"));
        assert!(!is_project_section_title("OPTIONS::"));
        assert!(!is_project_section_title("123"));
        assert!(!is_project_section_title(""));
    }

    #[test]
    fn managed_log_shaped_bullets_are_not_special_cased() {
        let sub_bullets = vec![
            bullet("🗓️ **SCHEDULE LOG**", AuthoredDepth::FIRST),
            bullet("Some entry", AuthoredDepth::NESTED),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(rendered.contents.matches("## ").count(), 1);
        assert!(rendered.contents.contains(
            "- [ ] #task 🗓️ **SCHEDULE LOG** [created::2026-09-20]\n"
        ));
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn named_tasks_render_ids_last_with_next_status_for_links() {
        pin_utc();
        let sub_bullets = vec![
            named_bullet(
                "Draft the resignation memo",
                AuthoredDepth::FIRST,
                "draft-memo",
                true,
            ),
            bullet("keep it short", AuthoredDepth::NESTED),
            named_bullet(
                "Call Morgan Stanley about the 401k",
                AuthoredDepth::FIRST,
                "call-ms",
                true,
            ),
            named_bullet(
                "Collect the equity paperwork",
                AuthoredDepth::FIRST,
                "equity-docs",
                false,
            ),
            bullet("FUTURE WORK", AuthoredDepth::FIRST),
            bullet("Revisit the severance terms", AuthoredDepth::NESTED),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(
            rendered.contents,
            "---\n\
             parent: \"[[cash]]\"\n\
             template: \"[[new_project]]\"\n\
             type: \"[[project]]\"\n\
             status: wip\n\
             created: 2026-09-20T14:31:07+0000\n\
             ---\n\
             \n\
             - [ ] #task #prj Finish the Google exit packet! #hide ^prj\n\
             \n\
             ## Tasks\n\
             \n\
             - [*] #task Draft the resignation memo [created::2026-09-20] ^draft-memo\n\
             \t- keep it short\n\
             - [*] #task Call Morgan Stanley about the 401k [created::2026-09-20] ^call-ms\n\
             - [ ] #task Collect the equity paperwork [created::2026-09-20] ^equity-docs\n\
             \n\
             ## Future Work\n\
             \n\
             - Revisit the severance terms\n"
        );
        assert_eq!(rendered.task_count, 3);
        assert_eq!(
            rendered
                .named_tasks
                .iter()
                .map(|task| (task.block_id.as_str(), task.link))
                .collect::<Vec<_>>(),
            vec![
                ("draft-memo", true),
                ("call-ms", true),
                ("equity-docs", false),
            ]
        );
        assert_eq!(
            rendered.named_tasks[0].task_line,
            "- [*] #task Draft the resignation memo [created::2026-09-20] ^draft-memo"
        );
        assert_eq!(rendered.named_tasks[0].text, "Draft the resignation memo");
    }

    #[test]
    fn named_all_caps_bullet_with_children_stays_a_task() {
        pin_utc();
        let sub_bullets = vec![
            named_bullet("FUTURE WORK", AuthoredDepth::FIRST, "future-1", true),
            bullet("Revisit the terms", AuthoredDepth::NESTED),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(!rendered.contents.contains("## Future Work"));
        assert!(rendered.contents.contains(
            "- [*] #task FUTURE WORK [created::2026-09-20] ^future-1\n\
             \t- Revisit the terms\n"
        ));
        assert!(rendered.sections.is_empty());
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn scheduled_project_renders_linked_tasks_as_blocked() {
        pin_utc();
        let sub_bullets = vec![
            named_bullet("Draft the memo", AuthoredDepth::FIRST, "draft", true),
            named_bullet("File the memo", AuthoredDepth::FIRST, "file", false),
        ];
        let mut input = base_input(&sub_bullets);
        input.scheduled = Some("2026-09-22");
        let rendered = render_project_note(&input);
        // PDG5: a future project date blocks every open task in the
        // note; only the linked task also asks for a Task Link.
        assert!(rendered.contents.contains(
            "- [?] #task Draft the memo [created::2026-09-20] #prj_is_blocked ^draft\n"
        ));
        assert!(rendered.contents.contains(
            "- [?] #task File the memo [created::2026-09-20] #prj_is_blocked ^file\n"
        ));
        assert!(rendered.task_line.starts_with("- [?] "));
        assert!(!rendered.task_line.contains("#prj_is_blocked"));
    }

    #[test]
    fn configured_cancelled_status_stays_closed_in_project_note() {
        pin_utc();
        let sub_bullets = vec![
            bullet("[>] Cancelled work", AuthoredDepth::FIRST),
            bullet("[ ] Open work", AuthoredDepth::FIRST),
        ];
        let mut input = base_input(&sub_bullets);
        input.scheduled = Some("2026-09-22");
        let rendered = render_project_note(&input);
        assert!(rendered
            .contents
            .contains("- [>] #task Cancelled work [created::2026-09-20]\n"));
        assert!(rendered.contents.contains(
            "- [?] #task Open work [created::2026-09-20] #prj_is_blocked\n"
        ));
    }

    #[test]
    fn unconfigured_status_stays_authored_in_scheduled_project_note() {
        pin_utc();
        let sub_bullets =
            vec![bullet("[!] Unknown status", AuthoredDepth::FIRST)];
        let mut input = base_input(&sub_bullets);
        input.scheduled = Some("2026-09-22");
        let rendered = render_project_note(&input);
        assert!(rendered
            .contents
            .contains("- [!] #task Unknown status [created::2026-09-20]\n"));
        assert!(!rendered.contents.contains("#prj_is_blocked"));
    }

    #[test]
    fn caret_task_keeps_its_authored_checkbox() {
        pin_utc();
        let sub_bullets = vec![
            named_bullet(
                "[/] Draft the memo",
                AuthoredDepth::FIRST,
                "draft",
                false,
            ),
            named_bullet(
                "Collect the paperwork",
                AuthoredDepth::FIRST,
                "paperwork",
                false,
            ),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains(
            "- [/] #task Draft the memo [created::2026-09-20] ^draft\n"
        ));
        assert!(rendered.contents.contains(
            "- [ ] #task Collect the paperwork [created::2026-09-20] ^paperwork\n"
        ));
    }

    #[test]
    fn named_task_keeps_an_existing_created_stamp_before_the_id() {
        pin_utc();
        let sub_bullets = vec![named_bullet(
            "Draft the memo [created::2026-09-18]",
            AuthoredDepth::FIRST,
            "draft",
            true,
        )];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains(
            "- [*] #task Draft the memo [created::2026-09-18] ^draft\n"
        ));
        assert_eq!(
            rendered.contents.matches("[created::2026-09-20]").count(),
            0
        );
    }

    #[test]
    fn task_rows_cover_unnamed_hidden_closed_and_placeholder() {
        pin_utc();
        let empty = render_project_note(&base_input(&[]));
        assert_eq!(empty.task_rows.len(), 1);
        assert!(empty.task_rows[0].placeholder);
        assert_eq!(empty.task_rows[0].text, PLACEHOLDER_TASK_BODY);
        assert_eq!(empty.task_rows[0].source_index, None);
        assert!(empty.task_rows[0].line > 0);
        assert!(!empty.task_rows[0].task_line.contains("^prj"));
        assert!(empty.contents.contains("^prj"));

        let sub_bullets = vec![
            bullet("Draft the memo", AuthoredDepth::FIRST),
            bullet("Private reminder #hide", AuthoredDepth::FIRST),
            bullet("[x] Closed work", AuthoredDepth::FIRST),
            bullet("[>] Cancelled work", AuthoredDepth::FIRST),
            named_bullet(
                "Call the bank",
                AuthoredDepth::FIRST,
                "call-bank",
                true,
            ),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(rendered.task_rows.len(), 5);
        assert_eq!(rendered.task_rows[0].text, "Draft the memo");
        assert_eq!(rendered.task_rows[0].status, ' ');
        assert_eq!(rendered.task_rows[0].block_id, None);
        assert!(!rendered.task_rows[0].link);
        assert!(!rendered.task_rows[0].hidden);
        assert_eq!(rendered.task_rows[0].source_index, Some(0));
        assert!(rendered.task_rows[1].hidden);
        assert_eq!(rendered.task_rows[2].status, 'x');
        assert_eq!(rendered.task_rows[3].status, '>');
        assert!(rendered.task_rows[4].link);
        assert_eq!(
            rendered.task_rows[4].block_id.as_deref(),
            Some("call-bank")
        );
        for row in &rendered.task_rows {
            assert_eq!(
                rendered
                    .contents
                    .lines()
                    .nth(row.line - 1)
                    .expect("task line exists"),
                row.task_line
            );
        }
        assert_eq!(rendered.named_tasks.len(), 1);
    }

    #[test]
    fn task_rows_keep_source_identity_when_tasks_section_merges_after() {
        pin_utc();
        let sub_bullets = vec![
            bullet("Draft the memo", AuthoredDepth::FIRST),
            bullet("TASKS", AuthoredDepth::FIRST),
            bullet("merged note", AuthoredDepth::NESTED),
            bullet("Collect paperwork", AuthoredDepth::FIRST),
            bullet("Collect paperwork", AuthoredDepth::FIRST),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        let texts: Vec<&str> = rendered
            .task_rows
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec!["Draft the memo", "Collect paperwork", "Collect paperwork"]
        );
        assert_eq!(rendered.task_rows[0].source_index, Some(0));
        assert_eq!(rendered.task_rows[1].source_index, Some(3));
        assert_eq!(rendered.task_rows[2].source_index, Some(4));
        assert!(!rendered
            .task_rows
            .iter()
            .any(|row| row.text == "merged note"));
        assert!(rendered.contents.contains("- merged note\n"));
        assert!(
            rendered.task_rows[1].line < rendered.task_rows[2].line,
            "duplicate descriptions keep note order"
        );
    }
}
