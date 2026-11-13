//! Shared fixtures for projects unit tests.
use super::*;

// Test-only sync wrapper moved from sync planning code.
fn plan_project_sync(
    project: &Project,
    has_open_subprojects: bool,
) -> ProjectPlan {
    plan_project_sync_at(
        project,
        has_open_subprojects,
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

fn apply_changes(contents: &str, changes: &[ProjectChange]) -> String {
    apply_project_changes(contents, changes).expect("apply edits")
}

mod edits;
mod parse;
mod surface;
mod sync;
