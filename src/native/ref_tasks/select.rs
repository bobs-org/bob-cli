//! Selection and residence: pure, reusable by `ref-sync-v2`.

use super::v1::TrackerHit;
use super::walk::LocatedRefTask;

/// Every diagnostic code this plan can emit.
pub(crate) const REF_TASK_DIAGNOSTIC_CODES: [&str; 7] = [
    "multiple_open_ref_tasks",
    "open_ref_task_in_done",
    "open_ref_without_task",
    "orphan_ref_task",
    "ref_task_outside_area",
    "parent_mismatch",
    "open_v1_tracker",
];

/// One selection diagnostic, converted to `ref_library::Diagnostic` on rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefTaskDiagnostic {
    pub code: &'static str,
    pub detail: String,
}

impl RefTaskDiagnostic {
    pub(crate) fn new(code: &'static str, detail: String) -> Self {
        Self { code, detail }
    }
}

/// The selected task for one ref note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selected {
    V2(LocatedRefTask),
}

/// What `decide_status` receives plus selection evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefTaskSelection {
    pub task: Option<Selected>,
    pub status_hits: Vec<TrackerHit>,
    pub v2: bool,
    pub residence: Option<String>,
    pub diagnostics: Vec<RefTaskDiagnostic>,
}

impl RefTaskSelection {
    pub(crate) fn empty() -> Self {
        Self {
            task: None,
            status_hits: Vec::new(),
            v2: false,
            residence: None,
            diagnostics: Vec::new(),
        }
    }
}

/// Whether a reading-task checkbox mark counts as open. Terminal marks are
/// `x`, `X`, `-`; every other mark, including `?` and unknown marks, is
/// open. Shared by selection and the v2 planner's v1-branch diagnostic.
pub(crate) fn is_open_mark(mark: char) -> bool {
    !matches!(mark, 'x' | 'X' | '-')
}

/// Apply the selection rules for one ref note.
///
/// `candidates` are this ref's v2 candidates (any order); `v1_hits` come
/// from `v1::find_trackers` on the ref note body. Pure and reusable by
/// `ref-sync-v2`. Terminal marks are `x`, `X`, `-`; every other mark,
/// including `?` and unknown marks, is open.
pub(crate) fn select_for_ref(
    candidates: &[LocatedRefTask],
    v1_hits: &[TrackerHit],
    parent: Option<&str>,
) -> RefTaskSelection {
    let v2 = !candidates.is_empty();
    let mut diagnostics = Vec::new();

    // Report-only: each open candidate under `done/`.
    for candidate in candidates {
        if candidate.archived && is_open_mark(candidate.mark) {
            diagnostics.push(RefTaskDiagnostic::new(
                "open_ref_task_in_done",
                format!(
                    "open reading task in archive {}:{}",
                    candidate.path,
                    candidate.line_index + 1,
                ),
            ));
        }
    }

    let open_outside: Vec<&LocatedRefTask> = candidates
        .iter()
        .filter(|c| !c.archived && is_open_mark(c.mark))
        .collect();

    let mut selection = RefTaskSelection {
        task: None,
        status_hits: Vec::new(),
        v2,
        residence: None,
        diagnostics: Vec::new(),
    };

    // Rule 1: exactly one open candidate outside done/.
    if open_outside.len() == 1 {
        let winner = open_outside[0].clone();
        selection.residence = winner.residence.clone();
        selection.status_hits = vec![TrackerHit {
            mark: winner.mark,
            line: winner.line.trim().to_string(),
        }];
        // Live-task residence check.
        if !winner.in_capture_target {
            diagnostics.push(RefTaskDiagnostic::new(
                "ref_task_outside_area",
                format!(
                    "reading task lives in {}, not an area, project, or inbox note; refile it with Ctrl+Shift+M",
                    winner.path,
                ),
            ));
        }
        selection.task = Some(Selected::V2(winner));
    } else if open_outside.len() >= 2 {
        // Rule 2: ambiguity; no selection, status falls back to frontmatter.
        let mut listed: Vec<&LocatedRefTask> = open_outside;
        listed.sort_by(|a, b| {
            a.path.cmp(&b.path).then(a.line_index.cmp(&b.line_index))
        });
        let locations = listed
            .iter()
            .map(|c| format!("{}:{}", c.path, c.line_index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let mut detail = format!(
            "{} open reading tasks claim this ref: {locations}",
            listed.len()
        );
        if let Some(id) = listed[0].block_id.as_deref()
            && listed
                .iter()
                .all(|task| task.block_id.as_deref() == Some(id))
        {
            let parent = parent
                .filter(|parent| !parent.is_empty())
                .map(|parent| format!("the ref's parent note ({parent})"))
                .unwrap_or_else(|| "the ref's parent note".to_string());
            detail.push_str(&format!(
                "; all are copies of one task ^{id} — keep the copy in {parent} and delete the others"
            ));
        }
        diagnostics
            .push(RefTaskDiagnostic::new("multiple_open_ref_tasks", detail));
        selection.status_hits = Vec::new();
        selection.task = None;
    } else {
        // Rule 3: closed candidates, newest by closed_on.
        let closed: Vec<&LocatedRefTask> = candidates
            .iter()
            .filter(|c| !is_open_mark(c.mark))
            .collect();
        if !closed.is_empty() {
            let mut sorted = closed;
            sorted.sort_by(|a, b| {
                match (&a.closed_on, &b.closed_on) {
                    (Some(ad), Some(bd)) => bd.cmp(ad),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
                .then_with(|| {
                    // Prefer outside done/.
                    (!a.archived).cmp(&(!b.archived)).reverse()
                })
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line_index.cmp(&b.line_index))
            });
            // Newest is last in ascending? We sorted descending by date
            // (newest first via bd.cmp(ad) reversed), so first is newest.
            let winner = sorted[0].clone();
            selection.residence = winner.residence.clone();
            selection.status_hits = vec![TrackerHit {
                mark: winner.mark,
                line: winner.line.trim().to_string(),
            }];
            selection.task = Some(Selected::V2(winner));
        } else if !v1_hits.is_empty() {
            // Rule 4: v1 passthrough.
            selection.status_hits = v1_hits.to_vec();
            // `task` stays None for v1 here. Let ref_library decide from
            // the carried hits whether a tracker is usable.
            selection.task = None;
        } else {
            // Rule 5: nothing.
            selection.status_hits = Vec::new();
            selection.task = None;
        }
    }

    // open_v1_tracker: an open v1 tracker emits even when v2 wins.
    let has_open_v1 = v1_hits.iter().any(|hit| is_open_mark(hit.mark));
    if has_open_v1 {
        let v2_claims = matches!(selection.task, Some(Selected::V2(_)));
        let mut detail = "open in-note ^ref tracker; run bob ref migrate-tasks to move it into its parent note".to_string();
        if v2_claims {
            detail.push_str(" (a v2 reading task also claims this ref)");
        }
        diagnostics.push(RefTaskDiagnostic::new("open_v1_tracker", detail));
    }

    selection.diagnostics = diagnostics;
    selection
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claimant(path: &str, block_id: Option<&str>) -> LocatedRefTask {
        LocatedRefTask {
            path: path.to_string(),
            line_index: 5,
            line: "- [ ] #task #ref [[ref/papers/x]]".to_string(),
            mark: ' ',
            block_id: block_id.map(str::to_string),
            archived: false,
            residence: None,
            closed_on: None,
            in_capture_target: true,
        }
    }

    #[test]
    fn copies_name_the_id_and_parent_without_selecting_a_winner() {
        for (parent, expected_parent) in [
            (None, "the ref's parent note"),
            (Some("sase"), "the ref's parent note (sase)"),
        ] {
            let tasks = [
                claimant("sase_agent_recovery.md", Some("ref-x")),
                claimant("sase.md", Some("ref-x")),
            ];
            let selection = select_for_ref(&tasks, &[], parent);
            assert!(selection.task.is_none());
            assert!(selection.status_hits.is_empty());
            assert_eq!(selection.diagnostics.len(), 1);
            assert_eq!(
                selection.diagnostics[0].code,
                "multiple_open_ref_tasks"
            );
            assert_eq!(selection.diagnostics[0].detail, format!(
                "2 open reading tasks claim this ref: sase.md:6, sase_agent_recovery.md:6; all are copies of one task ^ref-x — keep the copy in {expected_parent} and delete the others"
            ));
        }
    }

    #[test]
    fn distinct_or_missing_ids_do_not_get_a_copy_hint() {
        for ids in [
            [Some("ref-x"), Some("ref-y")],
            [Some("ref-x"), None],
            [None, None],
        ] {
            let tasks = [claimant("a.md", ids[0]), claimant("b.md", ids[1])];
            let selection = select_for_ref(&tasks, &[], Some("a"));
            assert_eq!(
                selection.diagnostics[0].detail,
                "2 open reading tasks claim this ref: a.md:6, b.md:6"
            );
        }
    }
}
