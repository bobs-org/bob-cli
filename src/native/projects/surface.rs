//! Shared project-surface predicate (`docs/task-dependencies.md` §12.10
//! Terms).
//!
//! The same EMPTY rule `bob projects sync` uses to surface `^prj`, over one
//! staged note text: recognition, plannability, the effective status, the
//! sync schedule, and the recounted effective-unhidden count come from the
//! factored sync helpers, so the close-gesture pull-in and sync cannot
//! drift. SUBOPEN reads the Projects host catalog first and falls back to
//! R's legacy sync-maintained marker line when no catalog exists.

use std::path::Path;

use chrono::NaiveDate;

use super::super::task_status_hooks::canonical_daily_date;
use super::catalog_rows::{parse_catalog, CatalogRead};
use super::*;
use crate::native::host_catalog::RegionDamage;

/// Surface state of one project note N (§12.10 Terms).
#[derive(Debug, Clone)]
pub(crate) struct ProjectSurface {
    /// 1-based line number of R's `^prj` line in the staged text: the
    /// planner's R identity (it edits exactly this line).
    pub(crate) prj_line_number: usize,
    /// SCHED(N): sync's project schedule date.
    pub(crate) sched_date: Option<NaiveDate>,
    /// Raw SCHED(N) date (`YYYY-MM-DD`) for row reporting.
    pub(crate) sched_raw: Option<String>,
    /// REMAINING(N): sync's effective-unhidden count on the staged text.
    pub(crate) remaining: usize,
    /// SUBOPEN(N): a catalog row or legacy marker lists an open child.
    pub(crate) subopen: bool,
    /// Open children in the catalog or `🧩 **Sub-projects:**` marker,
    /// counted with the same predicate as `subopen`.
    pub(crate) open_subprojects: usize,
}

impl ProjectSurface {
    /// EMPTY(N): no remaining work and no open sub-project.
    pub(crate) fn is_empty(&self) -> bool {
        should_surface_project(self.remaining, self.subopen)
    }
}

/// Surface state of the project note at `relative` with staged `contents`,
/// or `None` unless N is recognised and plannable per the Terms: outside
/// `done/`, not a canonical daily note, project frontmatter with
/// non-terminal status, exactly one open `^prj` task R with non-terminal
/// effective status, and a clean scan (no sync issue for the file).
pub(crate) fn project_surface(
    relative: &Path,
    contents: &str,
    today: NaiveDate,
) -> Option<ProjectSurface> {
    project_surface_inner(relative, contents, today, None)
}

/// [`project_surface`] with one sub-project read as closed: the catalog row
/// or legacy marker naming `closed_child` is ignored for SUBOPEN(N), whatever
/// its stored state (`docs/task-dependencies.md` §12.10 trigger (c)). The
/// entry matches by lowercased file stem.
pub(crate) fn project_surface_with_closed_child(
    relative: &Path,
    contents: &str,
    today: NaiveDate,
    closed_child: &Path,
) -> Option<ProjectSurface> {
    project_surface_inner(relative, contents, today, Some(closed_child))
}

fn project_surface_inner(
    relative: &Path,
    contents: &str,
    today: NaiveDate,
    closed_child: Option<&Path>,
) -> Option<ProjectSurface> {
    if crate::native::done_archive::is_archive_path(relative)
        || canonical_daily_date(relative).is_some()
    {
        return None;
    }
    let mut issues = Vec::new();
    let project = parse_project(relative, contents, &mut issues)?;
    if !issues.is_empty() {
        return None;
    }
    if project.status.is_terminal() {
        return None;
    }
    if project.prj_task.state != PrjTaskState::Open {
        return None;
    }
    let mut effective_status = project.status.clone();
    if let Some(target) = project.prj_task.target_status(&project.status)
        && !target.matches(&project.status)
    {
        effective_status = target.as_project_status();
    }
    if effective_status.is_terminal() {
        return None;
    }
    let sync_schedule = project_schedule_for_sync(&project, &effective_status);
    let remaining =
        effective_unhidden_count(&project, sync_schedule.as_ref(), today);
    let closed_stem = closed_child.and_then(|child| {
        child
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.to_ascii_lowercase())
    });
    let legacy_line = project
        .prj_task
        .sub_block
        .first_marker_line()
        .map(|line| line.trimmed_text.clone());
    let open_subprojects =
        read_subopen(contents, legacy_line.as_deref(), closed_stem.as_deref())
            .ok()?
            .open;
    let prj_line = project.task_lines.iter().find(|task| task.is_prj)?;
    Some(ProjectSurface {
        prj_line_number: prj_line.line_number,
        sched_date: sync_schedule.as_ref().map(|sched| sched.date),
        sched_raw: sync_schedule.as_ref().map(|sched| sched.raw.clone()),
        remaining,
        subopen: open_subprojects > 0,
        open_subprojects,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubopenSource {
    Catalog,
    Legacy,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SubopenRead {
    source: SubopenSource,
    open: usize,
}

/// Apply the shared catalog-first SUBOPEN rule. A damaged catalog is an
/// error so callers treat the project as unplannable; it never falls back to
/// the potentially stale legacy marker.
fn read_subopen(
    contents: &str,
    legacy_line: Option<&str>,
    closed_stem: Option<&str>,
) -> Result<SubopenRead, RegionDamage> {
    match parse_catalog(contents) {
        CatalogRead::Damaged(damage) => Err(damage),
        CatalogRead::Rows(rows) => Ok(SubopenRead {
            source: SubopenSource::Catalog,
            open: rows
                .iter()
                .filter(|row| {
                    row.state.is_open() && closed_stem != Some(row.key.as_str())
                })
                .count(),
        }),
        CatalogRead::Absent => {
            let Some(legacy_line) = legacy_line else {
                return Ok(SubopenRead {
                    source: SubopenSource::None,
                    open: 0,
                });
            };
            let displays = subproject_displays_in_marker_line(legacy_line);
            Ok(SubopenRead {
                source: SubopenSource::Legacy,
                open: displays
                    .iter()
                    .filter(|(link_name, display)| {
                        display.state.is_open()
                            && closed_stem != Some(link_name.as_str())
                    })
                    .count(),
            })
        }
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Vectors {
        vectors: Vec<Vector>,
    }

    #[derive(Deserialize)]
    struct Vector {
        name: String,
        text: String,
        #[serde(default)]
        closed_child: Option<String>,
        expect: Expected,
    }

    #[derive(Deserialize)]
    struct Expected {
        source: String,
        open: usize,
    }

    #[test]
    fn shared_project_catalog_vectors_match_rust_reader() {
        let vectors: Vectors = serde_json::from_str(include_str!(
            "../../../tests/fixtures/project_catalog/vectors.json"
        ))
        .expect("project catalog vectors parse");

        for vector in vectors.vectors {
            let mut issues = Vec::new();
            let project = parse_project(
                Path::new("Parent.md"),
                &vector.text,
                &mut issues,
            )
            .expect("fixture is a project note");
            assert!(issues.is_empty(), "{}: {issues:?}", vector.name);
            let legacy_line = project
                .prj_task
                .sub_block
                .first_marker_line()
                .map(|line| line.trimmed_text.as_str());
            let closed_stem = vector.closed_child.as_deref().map(|child| {
                Path::new(child)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .expect("closed child has a stem")
                    .to_ascii_lowercase()
            });

            let actual =
                read_subopen(&vector.text, legacy_line, closed_stem.as_deref());
            match vector.expect.source.as_str() {
                "damaged" => assert!(
                    actual.is_err(),
                    "{}: expected damaged, got {actual:?}",
                    vector.name
                ),
                source => {
                    let actual = actual.unwrap_or_else(|damage| {
                        panic!("{}: unexpected damage: {damage:?}", vector.name)
                    });
                    let actual_source = match actual.source {
                        SubopenSource::Catalog => "catalog",
                        SubopenSource::Legacy => "legacy",
                        SubopenSource::None => "none",
                    };
                    assert_eq!(actual_source, source, "{}", vector.name);
                    assert_eq!(
                        actual.open, vector.expect.open,
                        "{}",
                        vector.name
                    );
                }
            }
            if vector.expect.source == "damaged" {
                assert!(
                    project_surface(
                        Path::new("Parent.md"),
                        &vector.text,
                        NaiveDate::from_ymd_opt(2026, 10, 5).expect("date"),
                    )
                    .is_none(),
                    "{}: damaged catalog must make the project unplannable",
                    vector.name
                );
            }
        }
    }
}
