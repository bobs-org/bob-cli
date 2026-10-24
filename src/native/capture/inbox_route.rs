//! Batch-level detection of inbox tasks whose status changed in a capture.
use super::*;
use std::collections::BTreeSet;

use crate::native::{
    task_complete::is_inbox_note,
    task_move::{
        list_task_move_destinations, TaskMoveDestination, VaultSnapshot,
    },
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteReport {
    pub(super) supported: bool,
    pub(super) candidate_count: usize,
    pub(super) routed_count: usize,
    pub(super) kept_count: usize,
    pub(super) candidates: Vec<InboxRouteCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) destinations: Option<Vec<TaskMoveDestination>>,
}

impl InboxRouteReport {
    pub(super) fn empty() -> Self {
        Self {
            supported: true,
            candidate_count: 0,
            routed_count: 0,
            kept_count: 0,
            candidates: Vec::new(),
            destinations: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteCandidate {
    pub(super) key: String,
    pub(super) n: usize,
    pub(super) note_path: String,
    pub(super) inbox: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) answer: Option<InboxRouteAnswer>,
    pub(super) outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) moved: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct InboxRouteAnswer {
    pub(super) action: &'static str,
}

#[derive(Debug)]
struct CandidateDraft {
    relative_path: PathBuf,
    original_line: usize,
    original: note_tasks::NoteTask,
    current: note_tasks::NoteTask,
}

/// Detect candidates after all capture items and the successor net pass have
/// updated the staged planner. Destination discovery is deferred until at
/// least one candidate exists.
pub(super) fn detect_inbox_routes(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    dependency_ctx: &DependencyContext,
) -> Result<InboxRouteReport, CaptureError> {
    let mut drafts = Vec::new();
    let mut original_contents = BTreeMap::new();

    for file in &planner.files {
        if !file.existed || !file.present || file.original == file.current {
            continue;
        }
        let Ok(relative_path) = file.path.strip_prefix(bob_dir) else {
            continue;
        };
        let original_scan = note_tasks::scan(&file.original, settings);
        let current_scan = note_tasks::scan(&file.current, settings);
        if original_scan.tasks().is_empty() {
            continue;
        }
        let pairs = pair_task_lines(
            &file.original,
            &file.current,
            &original_scan,
            &current_scan,
        );
        let original_lines: Vec<&str> = file.original.lines().collect();
        let mut inbox_candidates = BTreeMap::new();

        for (&old_line, &new_line) in &pairs {
            let Some(old_task) = original_scan.task_at(old_line) else {
                continue;
            };
            let Some(new_task) = current_scan.task_at(new_line) else {
                continue;
            };
            if old_task.status_symbol == new_task.status_symbol {
                continue;
            }

            let candidate_line = if old_task.indentation.is_empty() {
                old_task.line_index
            } else {
                match column_zero_task_ancestor(
                    &original_lines,
                    &original_scan,
                    old_task.line_index,
                ) {
                    Some(ancestor) => ancestor,
                    None => continue,
                }
            };
            let Some(old_candidate) = original_scan.task_at(candidate_line)
            else {
                continue;
            };
            if old_candidate.block_id.as_deref() == Some("prj") {
                continue;
            }
            let Some(new_candidate_line) = pairs.get(&candidate_line).copied()
            else {
                continue;
            };
            let Some(new_candidate) = current_scan.task_at(new_candidate_line)
            else {
                continue;
            };
            if old_candidate.block_id.as_deref() == Some("prj") {
                continue;
            }
            // The root can be triggered by its own transition or by one of
            // its nested tasks. De-duplicate once per original block.
            inbox_candidates.insert(
                old_candidate.line_index,
                (old_candidate.clone(), new_candidate.clone()),
            );
        }

        for (original_line, (original, current)) in inbox_candidates {
            original_contents
                .entry(relative_path.to_path_buf())
                .or_insert_with(|| file.original.clone());
            drafts.push(CandidateDraft {
                relative_path: relative_path.to_path_buf(),
                original_line,
                original,
                current,
            });
        }
    }

    drafts.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(left.original_line.cmp(&right.original_line))
    });

    if drafts.is_empty() {
        return Ok(InboxRouteReport::empty());
    }
    let note_index = super::dependencies::staged_note_index(
        dependency_ctx,
        planner,
        bob_dir,
    );
    drafts.retain(|draft| {
        original_contents
            .get(&draft.relative_path)
            .is_some_and(|contents| {
                is_inbox_note(&draft.relative_path, contents, &note_index)
            })
    });
    if drafts.is_empty() {
        return Ok(InboxRouteReport::empty());
    }

    let mut digest_ordinals =
        std::collections::BTreeMap::<(PathBuf, String), usize>::new();
    let mut candidates = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.into_iter().enumerate() {
        let digest_ordinal = if draft.original.block_id.is_some() {
            1
        } else {
            let digest = hex::encode(Sha256::digest(
                draft.original.description.as_bytes(),
            ));
            let key = (draft.relative_path.clone(), digest[..8].to_string());
            let ordinal = digest_ordinals.entry(key).or_default();
            *ordinal += 1;
            *ordinal
        };
        let key = note_tasks::stable_task_key(
            &draft.relative_path,
            draft.original.block_id.as_deref(),
            &draft.original.description,
            settings.global_filter.as_str(),
            digest_ordinal,
        );
        let note_path = display_relative(&draft.relative_path);
        let inbox = draft
            .relative_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        candidates.push(InboxRouteCandidate {
            key,
            n: index + 1,
            note_path,
            inbox,
            block_id: draft.original.block_id.unwrap_or_default(),
            line: draft.current.line_index + 1,
            text: draft.original.description,
            previous_status_symbol: draft.original.status_symbol,
            previous_status_name: draft.original.status_name,
            status_symbol: draft.current.status_symbol,
            status_name: draft.current.status_name,
            answer: None,
            outcome: "pending",
            moved: None,
        });
    }

    let destinations = if candidates.is_empty() {
        None
    } else {
        let snapshot = destination_snapshot(bob_dir, planner)?;
        Some(list_task_move_destinations(&snapshot))
    };
    Ok(InboxRouteReport {
        supported: true,
        candidate_count: candidates.len(),
        routed_count: 0,
        kept_count: 0,
        candidates,
        destinations,
    })
}

fn pair_task_lines(
    original: &str,
    current: &str,
    original_scan: &note_tasks::NoteTaskScan,
    current_scan: &note_tasks::NoteTaskScan,
) -> BTreeMap<usize, usize> {
    use note_tasks::BlockIdLookup;

    let old_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = current.lines().collect();
    let (old_to_new, _) = line_maps(&old_lines, &new_lines);
    let mut pairs = BTreeMap::new();
    let mut used_current = BTreeSet::new();

    // Stable block IDs remain the strongest identity when their physical
    // line was rewritten, such as a checkbox transition.
    for task in original_scan.tasks() {
        if let Some(id) = task.block_id.as_deref()
            && let BlockIdLookup::Found(found) = current_scan.by_block_id(id)
            && used_current.insert(found.line_index)
        {
            pairs.insert(task.line_index, found.line_index);
        }
    }
    // Exact unchanged lines handle ID-less tasks and status-independent
    // edits elsewhere in the note.
    for task in original_scan.tasks() {
        if pairs.contains_key(&task.line_index) {
            continue;
        }
        if let Some(line) = old_to_new.get(&task.line_index).copied()
            && current_scan.task_at(line).is_some()
            && used_current.insert(line)
        {
            pairs.insert(task.line_index, line);
        }
    }
    // A checkbox edit makes its physical row a Myers replacement rather
    // than an equal line. Pair the task rows inside one-to-one replacement
    // hunks; additions and deletions never become candidates.
    let mut hunk: Option<(std::ops::Range<usize>, std::ops::Range<usize>)> =
        None;
    for operation in similar::capture_diff_slices(
        similar::Algorithm::Myers,
        &old_lines,
        &new_lines,
    ) {
        if operation.tag() == similar::DiffTag::Equal {
            pair_hunk_tasks(
                hunk.take(),
                original_scan,
                current_scan,
                &mut pairs,
                &mut used_current,
            );
            continue;
        }
        let old_range = operation.old_range();
        let new_range = operation.new_range();
        if let Some((old, new)) = hunk.as_mut() {
            old.end = old.end.max(old_range.end);
            new.end = new.end.max(new_range.end);
        } else {
            hunk = Some((old_range, new_range));
        }
    }
    pair_hunk_tasks(
        hunk,
        original_scan,
        current_scan,
        &mut pairs,
        &mut used_current,
    );
    pairs
}

fn pair_hunk_tasks(
    hunk: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    original_scan: &note_tasks::NoteTaskScan,
    current_scan: &note_tasks::NoteTaskScan,
    pairs: &mut BTreeMap<usize, usize>,
    used_current: &mut BTreeSet<usize>,
) {
    let Some((old_range, new_range)) = hunk else {
        return;
    };
    let old_tasks: Vec<_> = original_scan
        .tasks()
        .iter()
        .filter(|task| {
            old_range.contains(&task.line_index)
                && !pairs.contains_key(&task.line_index)
        })
        .collect();
    let new_tasks: Vec<_> = current_scan
        .tasks()
        .iter()
        .filter(|task| {
            new_range.contains(&task.line_index)
                && !used_current.contains(&task.line_index)
        })
        .collect();
    let mut old_by_description = BTreeMap::<String, Vec<_>>::new();
    let mut new_by_description = BTreeMap::<String, Vec<_>>::new();
    for task in old_tasks.iter().filter(|task| task.block_id.is_none()) {
        old_by_description
            .entry(task.description.clone())
            .or_default()
            .push(*task);
    }
    for task in new_tasks.iter().filter(|task| task.block_id.is_none()) {
        new_by_description
            .entry(task.description.clone())
            .or_default()
            .push(*task);
    }
    let mut paired_old = BTreeSet::new();
    let mut paired_new = BTreeSet::new();
    for (description, old_matches) in old_by_description {
        let Some(new_matches) = new_by_description.get(&description) else {
            continue;
        };
        if old_matches.len() != new_matches.len() {
            continue;
        }
        for (old_task, new_task) in old_matches.into_iter().zip(new_matches) {
            pairs.insert(old_task.line_index, new_task.line_index);
            used_current.insert(new_task.line_index);
            paired_old.insert(old_task.line_index);
            paired_new.insert(new_task.line_index);
        }
    }
    let remaining_old: Vec<_> = old_tasks
        .into_iter()
        .filter(|task| !paired_old.contains(&task.line_index))
        .collect();
    let remaining_new: Vec<_> = new_tasks
        .into_iter()
        .filter(|task| !paired_new.contains(&task.line_index))
        .collect();
    if remaining_old.len() == remaining_new.len() {
        for (old_task, new_task) in remaining_old.into_iter().zip(remaining_new)
        {
            pairs.insert(old_task.line_index, new_task.line_index);
            used_current.insert(new_task.line_index);
        }
    }
}

fn column_zero_task_ancestor(
    lines: &[&str],
    scan: &note_tasks::NoteTaskScan,
    nested_line: usize,
) -> Option<usize> {
    let nested_start = line_start_offset(lines, nested_line);
    scan.tasks()
        .iter()
        .filter(|task| {
            task.line_index < nested_line
                && task.indentation.is_empty()
                && task.block_end > nested_start
                && lines
                    .get(task.line_index)
                    .is_some_and(|line| collect_done::has_task_tag(line))
        })
        .max_by_key(|task| task.line_index)
        .map(|task| task.line_index)
}

fn line_start_offset(lines: &[&str], line: usize) -> usize {
    lines.iter().take(line).map(|line| line.len() + 1).sum()
}

fn destination_snapshot(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
) -> Result<VaultSnapshot, CaptureError> {
    let paths = collect_done::link_repair_markdown_files(bob_dir)
        .map_err(|error| CaptureError::io(error.to_string()))?;
    let mut notes = BTreeMap::new();
    for path in paths {
        let relative = path.strip_prefix(bob_dir).map_err(|error| {
            CaptureError::io(format!(
                "note {} is outside vault {}: {error}",
                path.display(),
                bob_dir.display()
            ))
        })?;
        let contents = fs::read_to_string(&path).map_err(|error| {
            CaptureError::io(format!(
                "failed to read {}: {error}",
                path.display()
            ))
        })?;
        notes.insert(relative.to_path_buf(), contents);
    }
    for file in &planner.files {
        let Ok(relative) = file.path.strip_prefix(bob_dir) else {
            continue;
        };
        if file.present {
            notes.insert(relative.to_path_buf(), file.current.clone());
        } else {
            notes.remove(relative);
        }
    }
    Ok(VaultSnapshot::from_notes(notes))
}

fn display_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-10-24";

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent directory"))
            .expect("create directory");
        fs::write(path, contents).expect("write note");
    }

    #[test]
    fn ir_nested_status_uses_root_and_idless_tasks_get_stable_keys() {
        // IR4: nested changes report their root; id-less duplicate keys are
        // numbered, and closing status changes remain route candidates.
        // IR5: a project filed under the inbox area is not itself an inbox.
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path().join("vault");
        write(&vault.join("inbox.md"), "# Inbox\n");
        let original = concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Parent task ^parent\n",
            "  - [ ] #task Nested child ^child\n",
            "- [*] #task Already Next ^next\n",
            "- [ ] #task Idless apples\n",
            "- [ ] #task Idless apples\n",
            "- [ ] #task Complete me ^complete\n",
        );
        let current = concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Parent task ^parent\n",
            "  - [*] #task Nested child ^child\n",
            "- [*] #task Already Next ^next\n",
            "- [/] #task Idless apples\n",
            "- [*] #task Idless apples\n",
            "- [x] #task Complete me ^complete\n",
            "- [ ] #task New this batch\n",
        );
        write(&vault.join("mac_inbox.md"), original);
        write(&vault.join("health.md"), "---\ntype: [[area]]\n---\n");
        let project_before = concat!(
            "---\n",
            "type: [[project]]\n",
            "status: wip\n",
            "parent: [[mac_inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Project task ^project-task\n",
        );
        let project_after = project_before
            .replace("- [ ] #task Project task", "- [*] #task Project task");
        write(&vault.join("projects/under-inbox.md"), project_before);

        let settings = note_tasks::read_settings(&vault);
        let mut planner = CaptureBatchPlanner::default();
        let inbox_path = vault.join("mac_inbox.md");
        planner.ensure_loaded(&inbox_path).expect("load inbox");
        planner
            .stage(&inbox_path, current.to_string())
            .expect("stage inbox update");
        let project_path = vault.join("projects/under-inbox.md");
        planner.ensure_loaded(&project_path).expect("load project");
        planner
            .stage(&project_path, project_after)
            .expect("stage project update");
        let today = chrono::NaiveDate::parse_from_str(TODAY, "%Y-%m-%d")
            .expect("today");
        let dependency_ctx = DependencyContext::new(&vault, today);
        let report =
            detect_inbox_routes(&planner, &vault, &settings, &dependency_ctx)
                .expect("route report");

        assert_eq!(report.candidate_count, 4, "{report:#?}");
        assert_eq!(report.candidates[0].key, "mac_inbox.md#^parent");
        assert_eq!(report.candidates[0].line, 6);
        assert_eq!(report.candidates[0].text, "Parent task");
        assert_eq!(report.candidates[0].previous_status_symbol, ' ');
        assert_eq!(report.candidates[0].status_symbol, ' ');
        let digest =
            hex::encode(Sha256::digest("Idless apples"))[..8].to_string();
        assert_eq!(report.candidates[1].key, format!("mac_inbox.md@{digest}"));
        assert_eq!(report.candidates[1].previous_status_symbol, ' ');
        assert_eq!(report.candidates[1].status_symbol, '/');
        assert_eq!(report.candidates[1].n, 2);
        assert_eq!(
            report.candidates[2].key,
            format!("mac_inbox.md@{digest}~2")
        );
        assert_eq!(report.candidates[2].status_symbol, '*');
        assert_eq!(report.candidates[3].key, "mac_inbox.md#^complete");
        assert_eq!(report.candidates[3].status_symbol, 'x');
        assert_eq!(report.candidates[0].answer, None);
        assert_eq!(report.candidates[0].outcome, "pending");
        let paths: Vec<_> = report
            .destinations
            .as_ref()
            .expect("destinations")
            .iter()
            .map(|destination| destination.relative_path.as_str())
            .collect();
        assert!(paths.contains(&"health.md"));
        assert!(paths.contains(&"projects/under-inbox.md"));
        assert!(!paths.contains(&"mac_inbox.md"));
        assert!(!paths.contains(&"inbox.md"));
    }
}
