//! Project side effects for `&` dependency captures on `^prj`
//! (`docs/task-dependencies.md` §13.6, §13.7, §13.10).
//!
//! `&` with a `^prj` dependent keeps the ordinary Depends-On write and,
//! when any new target is open, blocks the whole project note in the same
//! staged write: `^prj` and every open resident flip to `[?]`, residents
//! gain exactly one `#prj_is_blocked`, and their live links leave today's
//! open Pomodoros (emptied entries go with them). Same-note targets refuse:
//! a project cannot wait on its own tasks.

use super::*;
use crate::native::{
    capture_pomodoro_close::CloseVault,
    project_dependencies::{project_ancestors, ProjectContext},
    projects::edits::normalize_prj_is_blocked_tag,
    task_status_hooks::{self, FileScan, TaskLine},
    vault_links::NoteIndex,
};

/// The exact `^prj` block id the line model recognises. Matching stays
/// exact (like `ProjectContext`): resident side effects only run for
/// recognised projects, so a wider case-insensitive match would attach
/// project semantics to notes the model does not recognise.
pub(super) fn is_prj_dependent(block_id: &str) -> bool {
    block_id == "prj"
}

/// The recognised project context for an `&` dependent, with its
/// pre-write task parse for commitment transfer. `None` for ordinary
/// dependents and for `^prj` lines in notes the model does not
/// recognise (terminal, daily, or ambiguous notes).
pub(super) fn prj_context_for_dependent(
    ctx: &DependencyContext,
    dep_rel: &Path,
    dep_abs: &Path,
    contents: &str,
    dependent_block_id: &str,
) -> Option<(ProjectContext, Vec<TaskLine>)> {
    if !is_prj_dependent(dependent_block_id) {
        return None;
    }
    let tasks = task_status_hooks::parse_tasks(contents, ctx.settings());
    let context = ProjectContext::for_note(&FileScan {
        path: dep_abs.to_path_buf(),
        relative_path: dep_rel.to_path_buf(),
        contents: contents.to_string(),
        tasks,
    });
    if !context.recognised {
        return None;
    }
    let tasks = task_status_hooks::parse_tasks(contents, ctx.settings());
    Some((context, tasks))
}

fn note_is_recognised_project(
    relative: &Path,
    contents: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> bool {
    let tasks = task_status_hooks::parse_tasks(contents, settings);
    ProjectContext::for_note(&FileScan {
        path: relative.to_path_buf(),
        relative_path: relative.to_path_buf(),
        contents: contents.to_string(),
        tasks,
    })
    .recognised
}

/// Whether `target_rel`'s recognised-project ancestor chain reaches
/// `dep_rel` (PDG13): the target lives in a sub-project of the dependent.
fn target_is_descendant_of(
    dep_rel: &Path,
    target_rel: &Path,
    target_contents: &str,
    index: &NoteIndex,
    settings: &note_tasks::NoteTaskSettings,
    read: &mut impl FnMut(&Path) -> Option<String>,
) -> bool {
    project_ancestors(
        target_rel,
        target_contents,
        index,
        read,
        |relative, contents| {
            note_is_recognised_project(relative, contents, settings)
        },
    )
    .iter()
    .any(|ancestor| ancestor.as_path() == dep_rel)
}

/// Refuse same-note and descendant targets for a `^prj` dependent
/// (§13.6, PDG13): a project cannot wait on its own tasks or on a task
/// inside one of its sub-projects. Only new links refuse; pre-existing
/// hand-authored internal links stay on the line unprojected.
pub(super) fn refuse_internal_prj_targets(
    dep_rel: &Path,
    dep_desc: &str,
    prereqs: &[ResolvedPrerequisite],
    index: &NoteIndex,
    settings: &note_tasks::NoteTaskSettings,
    mut read: impl FnMut(&Path) -> Option<String>,
) -> Result<(), CaptureError> {
    for prereq in prereqs {
        if prereq.relative.as_path() == dep_rel {
            return Err(CaptureError::usage(format!(
                "cannot add dependency for {dep_desc}: ^{block} lives in the same project note (inside this project); a project cannot wait on its own tasks",
                block = prereq.block_id,
            )));
        }
        let Some(target_contents) = read(&prereq.relative) else {
            continue;
        };
        if target_is_descendant_of(
            dep_rel,
            &prereq.relative,
            &target_contents,
            index,
            settings,
            &mut read,
        ) {
            return Err(CaptureError::usage(format!(
                "cannot add dependency for {dep_desc}: ^{block} lives inside a sub-project of this project; a project cannot wait on its own tasks",
                block = prereq.block_id,
            )));
        }
    }
    Ok(())
}

/// Drop the field projection of pre-existing same-note and descendant
/// links on a `^prj` line (§13.6, PDG13): hand-authored internal
/// targets stay on the line but are never projected into
/// `[dependsOn::]`. Returns the kept count and any descendant note
/// paths for the `project_internal_dependency` warning.
pub(super) struct InternalPrjDeproject {
    pub(super) kept: usize,
    pub(super) descendants: Vec<String>,
}

pub(super) fn deproject_internal_prj_links(
    dep_rel: &Path,
    merged: &mut [MergedMember],
    index: &NoteIndex,
    settings: &note_tasks::NoteTaskSettings,
    mut read: impl FnMut(&Path) -> Option<String>,
) -> InternalPrjDeproject {
    let here = display_relative(dep_rel);
    let mut kept = 0usize;
    let mut descendants = Vec::new();
    for member in merged.iter_mut() {
        if member.projected_id.is_none() {
            continue;
        }
        if member.identity.0 == here {
            member.projected_id = None;
            kept += 1;
            continue;
        }
        let target_rel = PathBuf::from(&member.identity.0);
        let Some(target_contents) = read(&target_rel) else {
            continue;
        };
        if target_is_descendant_of(
            dep_rel,
            &target_rel,
            &target_contents,
            index,
            settings,
            &mut read,
        ) {
            member.projected_id = None;
            kept += 1;
            descendants.push(member.identity.0.clone());
        }
    }
    descendants.sort();
    descendants.dedup();
    InternalPrjDeproject { kept, descendants }
}

/// Strongest pre-capture lane among `^prj` and its open residents (§13.4
/// commitment transfer): each open prerequisite rises to at least this
/// lane. Closed tasks and non-residents never contribute.
pub(super) fn strongest_project_lane(
    context: &ProjectContext,
    tasks: &[TaskLine],
) -> Option<u8> {
    let mut best: Option<u8> = None;
    for task in tasks {
        let is_prj = task.block_id.as_deref() == Some("prj");
        if !is_prj && !context.is_resident(task) {
            continue;
        }
        if let Some(lane) = lane_for_status(task.status) {
            best = Some(best.map_or(lane, |known: u8| known.max(lane)));
        }
    }
    best
}

/// One staged resident edit: the resident id plus its rebuilt line.
pub(super) struct ResidentEdit {
    pub(super) block_id: Option<String>,
    pub(super) line: usize,
    pub(super) text: String,
}

/// Flip every open resident to `[?]` and ensure exactly one
/// `#prj_is_blocked` (PDT1, PDT8, PDT9). Closed lines are never touched
/// (PDT7); `^prj` is the caller's own write and is never tagged (PDT6).
/// Returns the staged line edits plus every open resident id for link
/// consumption, whether or not its line changed.
pub(super) fn plan_resident_block(
    context: &ProjectContext,
    tasks: &[TaskLine],
    staged_lines: &[String],
) -> (Vec<ResidentEdit>, Vec<String>) {
    let mut edits = Vec::new();
    let mut residents = Vec::new();
    // The caller may have inserted or removed a Depends-On child before
    // these lines. Rebuild the project context from the text being edited
    // so its fenced-line indices describe the same line numbering.
    let staged_contents = staged_lines.join("\n");
    let staged_context = ProjectContext::for_note(&FileScan {
        path: context.relative_path.clone(),
        relative_path: context.relative_path.clone(),
        contents: staged_contents,
        tasks: tasks.to_vec(),
    });
    for task in tasks {
        if !staged_context.is_resident(task) {
            continue;
        }
        let block_id = task.block_id.clone();
        if let Some(block_id) = &block_id {
            residents.push(block_id.clone());
        }
        let Some(line) = staged_lines.get(task.line_index) else {
            continue;
        };
        let blocked =
            replace_status_char(line, '?').unwrap_or_else(|| line.clone());
        let tagged = normalize_prj_is_blocked_tag(&blocked, true);
        if tagged != *line {
            edits.push(ResidentEdit {
                block_id,
                line: task.line_index,
                text: tagged,
            });
        }
    }
    (edits, residents)
}

#[derive(Debug, Default)]
pub(super) struct LiveLinkConsumption {
    pub(super) removed_links: usize,
    pub(super) pomodoro_refs: Vec<PomodoroBlockRef>,
}

pub(super) fn append_consumption_warning(
    warnings: &mut Vec<String>,
    removed_links: usize,
) {
    if removed_links == 0 {
        return;
    }
    let warning = format!(
        "removed {removed_links} live Pomodoro link(s) to newly Blocked task(s)"
    );
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

/// Remove every live Task Link to the supplied resolved identities from
/// today's open Pomodoros. Capture keeps child notes and only removes a
/// Pomodoro that this consumption emptied. A missing day file is nothing
/// to consume.
pub(super) fn consume_live_links(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    blocked: &std::collections::BTreeSet<(PathBuf, String)>,
) -> Result<LiveLinkConsumption, CaptureError> {
    let day_absolute = pomodoro::day_file_for(bob_dir);
    consume_live_links_from_day(planner, bob_dir, &day_absolute, blocked)
}

fn consume_live_links_from_day(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    day_absolute: &Path,
    blocked: &std::collections::BTreeSet<(PathBuf, String)>,
) -> Result<LiveLinkConsumption, CaptureError> {
    if blocked.is_empty() {
        return Ok(LiveLinkConsumption::default());
    }
    let before = match planner.current_contents(day_absolute)? {
        Some(staged) => staged,
        None => match fs::read_to_string(&day_absolute) {
            Ok(disk) => disk,
            Err(_) => return Ok(LiveLinkConsumption::default()),
        },
    };
    let before_lines = task_status_hooks::logical_lines(&before);
    let Some(section) = pomodoro::pomodoros_section_range(&before_lines) else {
        return Ok(LiveLinkConsumption::default());
    };
    let model = task_status_hooks::scan_pomodoros(&before_lines, section);
    let vault = SnapshotCloseVault::from_planner(planner, bob_dir);
    let settings = note_tasks::read_settings(bob_dir);
    let mut resolved = BTreeMap::new();
    let mut display_texts = BTreeMap::new();
    for reference in &model.all_references {
        let vault_path =
            match vault.resolve_target(day_absolute, &reference.target) {
                vault_links::LinkResolution::Found(path) => path,
                _ => continue,
            };
        let absolute = if vault_path.is_absolute() {
            vault_path
        } else {
            bob_dir.join(vault_path)
        };
        let Ok(relative) = absolute.strip_prefix(bob_dir) else {
            continue;
        };
        let relative = relative.to_path_buf();
        let Ok(Some(note_text)) = vault.read_latest(&absolute) else {
            continue;
        };
        let scan = note_tasks::scan(&note_text, &settings);
        let matches = scan
            .tasks()
            .iter()
            .filter(|task| {
                task.block_id.as_deref() == Some(&reference.block_id)
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            continue;
        }
        let identity = (relative.clone(), reference.block_id.clone());
        resolved.insert(
            reference.clone(),
            task_status_hooks::ResolvedReference {
                path: relative.clone(),
                statuses: matches
                    .iter()
                    .map(|task| task.status_symbol)
                    .collect(),
            },
        );
        if let Some(task) = matches.first() {
            let task_line = note_text
                .lines()
                .nth(task.line_index)
                .unwrap_or_default()
                .trim_end_matches('\r');
            let display = task_complete_display_text(
                task_line,
                &settings.global_filter,
                Some(&reference.block_id),
            );
            display_texts
                .entry(identity)
                .or_insert_with(|| flatten_block_wikilinks(&display));
        }
    }

    let item_pre = planner
        .item_pre_day(&day_absolute)
        .unwrap_or_else(|| before.clone());
    let item_pre_lines = task_status_hooks::logical_lines(&item_pre);
    let current_lines = task_status_hooks::logical_lines(&before);
    let (_, current_to_item) = line_maps(&item_pre_lines, &current_lines);
    let consumption = task_status_hooks::apply_blocked_consumption_with_options(
        &before,
        &resolved,
        blocked,
        &task_status_hooks::BlockedConsumptionOptions {
            keep_notes: true,
            only_emptied: true,
            display_texts: display_texts.clone(),
        },
    );
    if consumption.consumed.is_empty() {
        return Ok(LiveLinkConsumption::default());
    }
    let removed_entry_lines = consumption
        .removed_empty
        .iter()
        .map(|entry| entry.line_number)
        .collect::<std::collections::BTreeSet<_>>();
    let capture_entries = capture_pomodoros::scan(&before).entries;
    let mut pomodoro_refs = Vec::new();
    for entry in &consumption.removed_empty {
        let current_index = entry.line_number.saturating_sub(1);
        let item_index = current_to_item
            .get(&current_index)
            .copied()
            .unwrap_or(current_index);
        pomodoro_refs.push(PomodoroBlockRef::removed(item_index));
    }

    let day_relative = day_absolute
        .strip_prefix(bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_absolute.to_path_buf());
    let mut consumed_links = Vec::with_capacity(consumption.consumed.len());
    for consumed in &consumption.consumed {
        let current_link_line = consumed.line_number.saturating_sub(1);
        let current_pomodoro_line =
            consumed.pomodoro_line_number.saturating_sub(1);
        let line = current_to_item
            .get(&current_link_line)
            .copied()
            .unwrap_or(current_link_line)
            + 1;
        let pomodoro_line = current_to_item
            .get(&current_pomodoro_line)
            .copied()
            .unwrap_or(current_pomodoro_line)
            + 1;
        let entry = capture_entries
            .iter()
            .find(|entry| entry.line == consumed.pomodoro_line_number);
        let identity = (consumed.task_path.clone(), consumed.block_id.clone());
        let action = match consumed.action {
            task_status_hooks::BlockedLinkAction::Removed => {
                ConsumedLinkAction::Removed
            }
            task_status_hooks::BlockedLinkAction::TokenRemoved => {
                ConsumedLinkAction::TokenRemoved
            }
            task_status_hooks::BlockedLinkAction::NotesKept => {
                ConsumedLinkAction::NotesKept
            }
        };
        consumed_links.push(ConsumedLinkJson {
            day_file: display_relative(&day_relative),
            line,
            link: consumed.link.clone(),
            note: display_relative(&consumed.task_path),
            block_id: consumed.block_id.clone(),
            text: display_texts
                .get(&identity)
                .cloned()
                .unwrap_or_else(|| consumed.block_id.clone()),
            action,
            pomodoro: ConsumedLinkPomodoroJson {
                line: pomodoro_line,
                name: entry.and_then(|entry| entry.name.clone()),
                time_range: entry.and_then(|entry| entry.time_range.clone()),
                status: if entry.is_some_and(|entry| {
                    entry.state == capture_pomodoros::PomodoroState::Open
                        && entry.time_range.is_some()
                }) {
                    ConsumedPomodoroStatus::Running
                } else {
                    ConsumedPomodoroStatus::Queued
                },
                removed: removed_entry_lines
                    .contains(&consumed.pomodoro_line_number),
            },
        });
    }
    planner.record_consumed_links(consumed_links);
    planner.stage(day_absolute, consumption.contents)?;
    Ok(LiveLinkConsumption {
        removed_links: consumption.consumed.len(),
        pomodoro_refs,
    })
}

fn flatten_block_wikilinks(text: &str) -> String {
    let mut flattened = text.to_string();
    let spans = crate::native::task_dependencies::raw_wikilink_spans(text);
    for span in spans.into_iter().rev() {
        let inside = &text[span.open + 2..span.end - 2];
        let link_target = inside.split('|').next().unwrap_or(inside);
        let alias = inside.split_once('|').map(|(_, alias)| alias);
        let display = if let Some(alias) = alias {
            alias.to_string()
        } else if let Some((target, block_id)) =
            crate::native::task_dependencies::parse_block_link_inside(inside)
        {
            if target.is_empty() {
                block_id
            } else {
                target.rsplit('/').next().unwrap_or(&target).to_string()
            }
        } else {
            link_target.to_string()
        };
        flattened.replace_range(span.open..span.end, &display);
    }
    flattened
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn settings() -> note_tasks::NoteTaskSettings {
        note_tasks::NoteTaskSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', task_status_hooks::TaskStatusType::Todo),
                ('x', task_status_hooks::TaskStatusType::Done),
                ('X', task_status_hooks::TaskStatusType::Done),
                ('/', task_status_hooks::TaskStatusType::InProgress),
                ('*', task_status_hooks::TaskStatusType::OnHold),
                ('?', task_status_hooks::TaskStatusType::OnHold),
                ('-', task_status_hooks::TaskStatusType::Cancelled),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        }
    }

    fn write_note(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(path, contents).expect("write note");
    }

    #[test]
    fn flatten_block_links_uses_plain_alias_or_target_text() {
        assert_eq!(
            flatten_block_wikilinks(
                "Read [[projects/other#^x|the handoff]] and [[#^local]]"
            ),
            "Read the handoff and local"
        );
    }

    #[test]
    fn consumption_resolves_alias_basename_path_case_and_local_links_by_identity(
    ) {
        let vault = tempfile::tempdir().expect("temporary vault");
        let root = vault.path();
        write_note(
            root,
            ".obsidian/plugins/obsidian-tasks-plugin/data.json",
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Ready","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"}
                ],
                "customStatuses": [
                  {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
                ]
              }
            }"##,
        );
        write_note(
            root,
            "projects/Jobs.md",
            "- [?] #task Ship the handoff [[other#^target|block link]] ^blocked\n",
        );
        let day_relative = PathBuf::from("2026/20261014.md");
        let day_absolute = root.join(&day_relative);
        write_note(
            root,
            "2026/20261014.md",
            concat!(
                "## Pomodoros\n\n",
                "- [ ] Alias\n",
                "  - [[projects/Jobs#^blocked|friendly]]\n",
                "- [ ] Basename\n",
                "  - [[Jobs#^blocked]]\n",
                "- [ ] Path\n",
                "  - [[projects/Jobs#^blocked]]\n",
                "- [ ] Case\n",
                "  - [[jobs#^blocked]]\n",
                "- [ ] Local\n",
                "  - [[#^local]]\n",
                "- [ ] Unrelated placeholder\n",
                "- [x] Closed\n",
                "  - [[Jobs#^blocked]]\n",
                "- [ ] Struck\n",
                "  - ~~[[Jobs#^blocked]]~~\n",
                "## Tasks\n",
                "- [?] #task Local blocked task ^local\n",
            ),
        );
        let mut planner = CaptureBatchPlanner::default();
        let blocked = BTreeSet::from([
            (PathBuf::from("projects/Jobs.md"), "blocked".to_string()),
            (day_relative, "local".to_string()),
        ]);

        let result = consume_live_links_from_day(
            &mut planner,
            root,
            &day_absolute,
            &blocked,
        )
        .expect("consume by resolved identity");
        let consumed = planner.take_consumed_links();
        assert_eq!(result.removed_links, 5, "resolved rows: {consumed:?}");
        let after =
            planner.peek_text(&day_absolute).expect("staged daily note");
        assert!(after.contains("- [ ] Unrelated placeholder\n"));
        assert!(after.contains("- [x] Closed\n  - [[Jobs#^blocked]]\n"));
        assert!(after.contains("- [ ] Struck\n  - ~~[[Jobs#^blocked]]~~\n"));
        assert_eq!(consumed.len(), 5);
        assert!(consumed
            .iter()
            .any(|item| item.link == "[[projects/Jobs#^blocked|friendly]]"));
        assert!(consumed.iter().any(|item| item.link == "[[#^local]]"));
        assert!(consumed
            .iter()
            .any(|item| { item.text == "Ship the handoff block link" }));
    }

    #[test]
    fn removed_refs_and_consumed_lines_use_the_items_pre_state_coordinates() {
        let vault = tempfile::tempdir().expect("temporary vault");
        let root = vault.path();
        write_note(
            root,
            ".obsidian/plugins/obsidian-tasks-plugin/data.json",
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Ready","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"}
                ],
                "customStatuses": [
                  {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
                ]
              }
            }"##,
        );
        write_note(root, "tasks.md", "- [?] #task Blocked ^blocked\n");
        let day_relative = PathBuf::from("2026/20261014.md");
        let day_absolute = root.join(&day_relative);
        let pre = concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[tasks#^blocked]]\n",
        );
        write_note(root, "2026/20261014.md", pre);
        let staged = concat!(
            "## Pomodoros\n\n",
            "- [ ] Earlier\n",
            "  - [[tasks#^keep]]\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[tasks#^blocked]]\n",
        );
        let mut planner = CaptureBatchPlanner::default();
        planner.begin_item(Some(pre.to_string()));
        planner
            .stage(&day_absolute, staged.to_string())
            .expect("stage earlier item edit");
        let blocked = BTreeSet::from([(
            PathBuf::from("tasks.md"),
            "blocked".to_string(),
        )]);

        let result = consume_live_links_from_day(
            &mut planner,
            root,
            &day_absolute,
            &blocked,
        )
        .expect("consume shifted entry");
        assert_eq!(result.removed_links, 1);
        assert_eq!(
            result.pomodoro_refs[0].before,
            PomodoroBlockBefore::Removed(2),
            "the deleted session ref uses the item pre-state headline line"
        );
        let consumed = planner.take_consumed_links();
        assert_eq!(consumed[0].line, 4);
        assert_eq!(consumed[0].pomodoro.line, 3);
    }

    fn project_context(body: &str) -> (ProjectContext, Vec<TaskLine>) {
        let contents =
            format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}");
        let tasks = task_status_hooks::parse_tasks(&contents, &settings());
        let context = ProjectContext::for_note(&FileScan {
            path: PathBuf::from("/vault/proj.md"),
            relative_path: PathBuf::from("proj.md"),
            contents: contents.clone(),
            tasks,
        });
        assert!(context.recognised, "fixture must be a project");
        let tasks = task_status_hooks::parse_tasks(&contents, &settings());
        (context, tasks)
    }

    #[test]
    fn resident_block_flips_tags_and_skips_prj_and_closed() {
        // PDT1/PDT6/PDT7/PDT8 (§13.18).
        let (context, tasks) = project_context(concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [ ] #task Work ^work\n",
            "- [?] #task Waiting #prj_is_blocked ^waiting\n",
            "- [x] #task Done #prj_is_blocked ^done\n",
        ));
        let contents = "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Ship #hide ^prj\n- [ ] #task Work ^work\n- [?] #task Waiting #prj_is_blocked ^waiting\n- [x] #task Done #prj_is_blocked ^done\n";
        let staged: Vec<String> =
            contents.lines().map(str::to_string).collect();
        let (edits, residents) = plan_resident_block(&context, &tasks, &staged);
        assert_eq!(
            residents,
            vec!["work".to_string(), "waiting".to_string()],
            "open residents only: no ^prj, no closed lines"
        );
        assert_eq!(edits.len(), 1, "only the untagged resident changes");
        assert_eq!(edits[0].block_id.as_deref(), Some("work"));
        assert!(
            edits[0].text.starts_with("- [?] "),
            "resident flips to Blocked: {}",
            edits[0].text
        );
        assert_eq!(
            edits[0].text.matches("#prj_is_blocked").count(),
            1,
            "exactly one tag: {}",
            edits[0].text
        );
        assert!(
            edits[0].text.ends_with("^work"),
            "the tag lands before the block id: {}",
            edits[0].text
        );
    }

    #[test]
    fn resident_block_edits_idless_rows_and_rechecks_shifted_fences() {
        // PDT1/PDT7 (§13.18): id-less open residents still receive their
        // immediate status/tag write; IDs are only needed for link cleanup.
        let body = concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "```md\n",
            "- [ ] #task Fenced example ^fenced\n",
            "```\n",
            "- [ ] #task Unnamed resident\n",
            "- [ ] #task Named resident ^named\n",
        );
        let (context, _) = project_context(body);
        let contents =
            format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}");
        let mut staged =
            contents.lines().map(str::to_string).collect::<Vec<_>>();
        let prj_index = staged
            .iter()
            .position(|line| line.contains("^prj"))
            .expect("project task line");
        staged.insert(
            prj_index + 1,
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]".to_string(),
        );
        let staged_text = staged.join("\n");
        let staged_tasks =
            task_status_hooks::parse_tasks(&staged_text, &settings());
        let (edits, residents) =
            plan_resident_block(&context, &staged_tasks, &staged);
        assert_eq!(residents, vec!["named".to_string()]);
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].block_id, None);
        assert_eq!(edits[1].block_id.as_deref(), Some("named"));
        assert!(edits.iter().all(|edit| edit.text.starts_with("- [?] ")));
        assert!(edits
            .iter()
            .all(|edit| edit.text.contains("#prj_is_blocked")));
        assert!(!edits
            .iter()
            .any(|edit| edit.text.contains("Fenced example")));
    }

    #[test]
    fn resident_after_prj_before_fence_is_not_hidden_by_stale_fence_range() {
        let body = concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [ ] #task Resident ^resident\n",
            "```md\n",
            "- [ ] #task Fenced example ^fenced\n",
            "```\n",
        );
        let (context, _) = project_context(body);
        let contents =
            format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}");
        let mut staged =
            contents.lines().map(str::to_string).collect::<Vec<_>>();
        let prj_index = staged
            .iter()
            .position(|line| line.contains("^prj"))
            .expect("project task line");
        staged.insert(
            prj_index + 1,
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]".to_string(),
        );
        let staged_text = staged.join("\n");
        let staged_tasks =
            task_status_hooks::parse_tasks(&staged_text, &settings());
        let staged_context = ProjectContext::for_note(&FileScan {
            path: PathBuf::from("/vault/proj.md"),
            relative_path: PathBuf::from("proj.md"),
            contents: staged_text.clone(),
            tasks: staged_tasks.clone(),
        });
        assert!(
            staged_context.recognised,
            "frontmatter and ^prj stay a project after the Depends-On insert"
        );
        let (edits, residents) =
            plan_resident_block(&context, &staged_tasks, &staged);
        assert_eq!(residents, vec!["resident".to_string()]);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].block_id.as_deref(), Some("resident"));
        assert!(edits[0].text.contains("#prj_is_blocked"));
        assert!(
            !edits
                .iter()
                .any(|edit| edit.text.contains("Fenced example")),
            "the fenced example stays unedited"
        );
    }

    #[test]
    fn strongest_lane_ignores_closed_and_outsiders() {
        let (context, tasks) = project_context(concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [/] #task Grind ^grind\n",
            "- [x] #task Done ^done\n",
        ));
        assert_eq!(strongest_project_lane(&context, &tasks), Some(2));
        let (context, tasks) = project_context(concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [ ] #task Work ^work\n",
        ));
        assert_eq!(strongest_project_lane(&context, &tasks), Some(0));
    }

    #[test]
    fn prj_gate_matches_only_recognised_projects() {
        assert!(is_prj_dependent("prj"));
        assert!(!is_prj_dependent("work"));
        // Case variants are not line-model projects: the capture
        // picker's wider reservation does not drive side effects.
        assert!(!is_prj_dependent("PRJ"));
    }

    fn cascade_notes() -> BTreeMap<PathBuf, String> {
        BTreeMap::from([
            (
                PathBuf::from("parent.md"),
                "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Parent #hide ^prj\n".to_string(),
            ),
            (
                PathBuf::from("child.md"),
                "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n- [ ] #task Child #hide ^prj\n- [ ] #task Work ^work\n".to_string(),
            ),
        ])
    }

    #[test]
    fn descendant_notes_are_internal_prj_targets() {
        // PDG13: a target whose ancestor chain reaches the dependent.
        let notes = cascade_notes();
        let index = crate::native::vault_links::NoteIndex::from_paths(
            notes.keys().cloned(),
        );
        let child = notes.get(Path::new("child.md")).cloned().unwrap();
        assert!(target_is_descendant_of(
            Path::new("parent.md"),
            Path::new("child.md"),
            &child,
            &index,
            &settings(),
            &mut |relative| notes.get(relative).cloned(),
        ));
        assert!(!target_is_descendant_of(
            Path::new("parent.md"),
            Path::new("parent.md"),
            notes.get(Path::new("parent.md")).unwrap(),
            &index,
            &settings(),
            &mut |relative| notes.get(relative).cloned(),
        ));
    }
}
