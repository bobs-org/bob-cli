//! Surface-predicate agreement tests (`docs/task-dependencies.md` §12.10
//! Terms): sync's surface decision and `project_surface` agree on the
//! PP2–PP6, PP20, PP26 and PP28 fixtures when no child notes exist. The one
//! deliberate divergence is PP6's marker proxy: with no child notes sync
//! would surface, while the pull-in trusts an open marker child.

use super::*;
use chrono::NaiveDate;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 5).expect("date")
}

fn project_note(body: &str) -> String {
    format!("---\ntype: \"[[project]]\"\nstatus: wip\n---\n{body}")
}

fn parse(path: &str, contents: &str) -> (Project, Vec<ScanIssue>) {
    let mut issues = Vec::new();
    let project = parse_project(Path::new(path), contents, &mut issues)
        .expect("project note");
    (project, issues)
}

fn surface_of(path: &str, contents: &str) -> Option<ProjectSurface> {
    project_surface(Path::new(path), contents, today())
}

fn sync_changes(project: &Project) -> Vec<ProjectChange> {
    plan_project_sync_at(project, &[], today()).changes
}

fn has_remove_hide(changes: &[ProjectChange]) -> bool {
    changes
        .iter()
        .any(|change| matches!(change, ProjectChange::RemoveHideTag))
}

fn has_add_hide(changes: &[ProjectChange]) -> bool {
    changes
        .iter()
        .any(|change| matches!(change, ProjectChange::AddHideTag { .. }))
}

// PP2: an open sibling keeps `^prj` hidden in sync and blocks EMPTY.
#[test]
fn surface_agrees_open_sibling_hides() {
    let contents = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task Other work [id:: other] ^other\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let (project, issues) = parse("Proj.md", &contents);
    assert!(issues.is_empty());
    assert!(!has_remove_hide(&sync_changes(&project)));
    let surface = surface_of("Proj.md", &contents).expect("surface");
    assert_eq!(surface.remaining, 1);
    assert!(!surface.is_empty());
}

// PP3: a `[?]` sibling is remaining work in both engines.
#[test]
fn surface_agrees_blocked_sibling_hides() {
    let contents = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [?] #task Blocked work [id:: blocked] ^blocked\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let (project, issues) = parse("Proj.md", &contents);
    assert!(issues.is_empty());
    assert!(!has_remove_hide(&sync_changes(&project)));
    let surface = surface_of("Proj.md", &contents).expect("surface");
    assert_eq!(surface.remaining, 1);
    assert!(!surface.is_empty());
}

// PP4: a hidden sibling surfaces in sync and empties the note.
#[test]
fn surface_agrees_hidden_sibling_surfaces() {
    let contents = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task Hidden helper #hide ^helper\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let (project, issues) = parse("Proj.md", &contents);
    assert!(issues.is_empty());
    assert!(has_remove_hide(&sync_changes(&project)));
    let surface = surface_of("Proj.md", &contents).expect("surface");
    assert_eq!(surface.remaining, 0);
    assert!(!surface.subopen);
    assert!(surface.is_empty());
}

// PP5: fenced lines never count; blockquoted and custom-mark lines do.
#[test]
fn surface_agrees_fence_quote_and_custom_counts() {
    let fenced = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "```\n",
        "- [ ] #task Fenced helper ^fenced\n",
        "```\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let (project, issues) = parse("Proj.md", &fenced);
    assert!(issues.is_empty());
    assert!(has_remove_hide(&sync_changes(&project)));
    let surface = surface_of("Proj.md", &fenced).expect("surface");
    assert!(surface.is_empty());

    for sibling in [
        "> - [ ] #task Quoted helper ^quoted\n",
        "- [>] #task Custom helper ^custom\n",
    ] {
        let contents = project_note(&format!(
            "- [x] #task Fix pump [id:: fix-a] ^a-fix\n{sibling}- [ ] #task #prj Ship it! #hide ^prj\n"
        ));
        let (project, issues) = parse("Proj.md", &contents);
        assert!(issues.is_empty(), "{sibling}");
        assert!(!has_remove_hide(&sync_changes(&project)), "{sibling}");
        let surface = surface_of("Proj.md", &contents).expect("surface");
        assert_eq!(surface.remaining, 1, "{sibling}");
        assert!(!surface.is_empty(), "{sibling}");
    }
}

// PP6: an open marker child keeps SUBOPEN even with no child notes, while
// sync (which reads child notes, not the marker) would surface. Both agree
// once every marker child is ✅/❌.
#[test]
fn surface_marker_proxy_divergence_and_agreement() {
    for marker in [
        "  - 🧩 **Sub-projects:** [[child]]\n",
        "  - 🧩 **Sub-projects:** ~~[[child]]~~\n",
    ] {
        let contents = project_note(&format!(
            "- [x] #task Fix pump [id:: fix-a] ^a-fix\n\
             - [ ] #task #prj Ship it! #hide ^prj\n\
             {marker}"
        ));
        let (project, issues) = parse("Proj.md", &contents);
        assert!(issues.is_empty(), "{marker}");
        assert!(has_remove_hide(&sync_changes(&project)), "{marker}");
        let surface = surface_of("Proj.md", &contents).expect("surface");
        assert!(surface.subopen, "{marker}");
        assert!(!surface.is_empty(), "{marker}");
    }

    for marker in [
        "  - 🧩 **Sub-projects:** ~~[[child]]~~ ✅\n",
        "  - 🧩 **Sub-projects:** ~~[[child]]~~ ❌\n",
    ] {
        let contents = project_note(&format!(
            "- [x] #task Fix pump [id:: fix-a] ^a-fix\n\
             - [ ] #task #prj Ship it! #hide ^prj\n\
             {marker}"
        ));
        let (project, issues) = parse("Proj.md", &contents);
        assert!(issues.is_empty(), "{marker}");
        assert!(has_remove_hide(&sync_changes(&project)), "{marker}");
        let surface = surface_of("Proj.md", &contents).expect("surface");
        assert!(!surface.subopen, "{marker}");
        assert!(surface.is_empty(), "{marker}");
    }
}

// PP20: terminal projects, duplicate `^prj`, `done/` notes, canonical daily
// notes, and scan issues are never surfaceable.
#[test]
fn surface_rejects_unplannable_notes() {
    let terminal = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ))
    .replacen("status: wip", "status: done", 1);
    let two_prj = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
        "- [ ] #task #prj Second criteria #hide ^prj\n",
    ));
    let plain = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let two_dates = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide [scheduled:: 2026-10-06] [scheduled:: 2026-10-07] ^prj\n",
    ));
    for (path, contents) in [
        ("Proj.md", terminal),
        ("Proj.md", two_prj),
        ("done/Proj.md", plain.clone()),
        ("2026/20261005.md", plain.clone()),
        ("Proj.md", two_dates),
    ] {
        assert!(surface_of(path, &contents).is_none(), "{path}");
    }
    // The scan-issue fixture is one sync cannot plan either.
    let (_, issues) = parse(
        "Proj.md",
        &project_note(concat!(
            "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
            "- [ ] #task #prj Ship it! #hide [scheduled:: 2026-10-06] [scheduled:: 2026-10-07] ^prj\n",
        )),
    );
    assert!(!issues.is_empty());
}

// PP26: under a project date a hidden sibling counts (the dated recount),
// so sync keeps `^prj` hidden and the note is not empty. Without the date
// both engines surface.
#[test]
fn surface_agrees_dated_recount() {
    let dated = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task Other work #hide ^other\n",
        "- [ ] #task #prj Ship it! #hide [scheduled:: 2026-10-05] ^prj\n",
    ));
    let (project, issues) = parse("Proj.md", &dated);
    assert!(issues.is_empty());
    assert!(!has_remove_hide(&sync_changes(&project)));
    let surface = surface_of("Proj.md", &dated).expect("surface");
    assert_eq!(surface.remaining, 1);
    assert_eq!(surface.sched_date, Some(today()));
    assert!(!surface.is_empty());

    let undated = project_note(concat!(
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task Other work #hide ^other\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    ));
    let (project, issues) = parse("Proj.md", &undated);
    assert!(issues.is_empty());
    assert!(has_remove_hide(&sync_changes(&project)));
    let surface = surface_of("Proj.md", &undated).expect("surface");
    assert!(surface.is_empty());
}

// PP28: a leftover frontmatter future schedule is SCHED(N): sync leaves the
// hide tag alone and the pull-in still-blocks, so neither surfaces.
#[test]
fn surface_agrees_frontmatter_schedule_defers() {
    let contents = concat!(
        "---\ntype: \"[[project]]\"\nstatus: wip\nscheduled: 2026-10-13\n---\n",
        "- [x] #task Fix pump [id:: fix-a] ^a-fix\n",
        "- [ ] #task #prj Ship it! #hide ^prj\n",
    );
    let (project, issues) = parse("Proj.md", contents);
    assert!(issues.is_empty());
    let changes = sync_changes(&project);
    assert!(!has_remove_hide(&changes));
    assert!(!has_add_hide(&changes));
    let surface = surface_of("Proj.md", contents).expect("surface");
    assert!(surface.is_empty());
    assert_eq!(surface.sched_date, NaiveDate::from_ymd_opt(2026, 10, 13));
    assert_eq!(surface.sched_raw.as_deref(), Some("2026-10-13"));
}
