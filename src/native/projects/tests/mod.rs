//! Shared fixtures for projects unit tests.
use super::*;

// Test-only sync wrapper moved from sync planning code.
fn plan_project_sync(
    project: &Project,
    subproject_children: &[SubprojectEntry],
) -> ProjectPlan {
    plan_project_sync_at(
        project,
        subproject_children,
        bob_env::current_datetime().date(),
    )
}

fn parse_clean_project(path: &str, contents: &str) -> Project {
    let mut issues = Vec::new();
    let project = parse_project(Path::new(path), contents, &mut issues)
        .expect("project note");
    assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    project
}

fn subproject(stem: &str, state: SubprojectState) -> SubprojectEntry {
    SubprojectEntry {
        link_name: stem.to_ascii_lowercase(),
        stem: stem.to_string(),
        state,
        future_scheduled: false,
    }
}

fn future_subproject(stem: &str, state: SubprojectState) -> SubprojectEntry {
    SubprojectEntry {
        future_scheduled: true,
        ..subproject(stem, state)
    }
}

fn open_subproject(stem: &str) -> SubprojectEntry {
    subproject(stem, SubprojectState::Open)
}

fn done_subproject(stem: &str) -> SubprojectEntry {
    subproject(stem, SubprojectState::Done)
}

fn canceled_subproject(stem: &str) -> SubprojectEntry {
    subproject(stem, SubprojectState::Canceled)
}

fn apply_changes(contents: &str, changes: &[ProjectChange]) -> String {
    apply_project_changes(contents, changes, None).expect("apply edits")
}

fn apply_subproject_changes(
    contents: &str,
    changes: &[ProjectChange],
    desired_subprojects: &[SubprojectEntry],
) -> String {
    apply_project_changes(contents, changes, Some(desired_subprojects))
        .expect("apply edits")
}

mod edits;
mod parse;
mod sync;
