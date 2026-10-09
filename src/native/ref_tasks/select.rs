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
    V1(TrackerHit),
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
        let detail = listed
            .iter()
            .map(|c| format!("{}:{}", c.path, c.line_index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        diagnostics.push(RefTaskDiagnostic::new(
            "multiple_open_ref_tasks",
            format!(
                "{} open reading tasks claim this ref: {detail}",
                listed.len()
            ),
        ));
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
            // `task` stays None for v1 here; the caller maps the usable
            // single hit to Selected::V1 when decide_status finds it usable.
            // Keep task None and let ref_library decide; but carry hits.
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
