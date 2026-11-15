//! Pure per-host planner for generated nested area catalogs.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::{AreaGraph, AreaIssue};
use crate::native::{
    host_catalog::{self, EditState, FileEdit, HostOutcome, AREAS},
    notebooks::{
        catalog::inert_text,
        snapshot::{LiteKind, VaultSnapshot},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKind {
    Root,
    Area,
    Project,
}

impl HostKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Area => "area",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogState {
    Missing,
    Stale,
    Current,
    Damaged,
}

impl CatalogState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Current => "current",
            Self::Damaged => "damaged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowRecord {
    pub(crate) path: PathBuf,
    pub(crate) link: String,
    /// Depth relative to the catalog host: direct children are zero.
    pub(crate) depth: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct HostPlan {
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    pub(crate) kind: HostKind,
    pub(crate) state: CatalogState,
    pub(crate) rows: Vec<RowRecord>,
    pub(crate) issues: Vec<AreaIssue>,
    #[allow(dead_code)] // consumed by the areas-sync writer
    pub(crate) edit: Option<FileEdit>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogPlan {
    pub(crate) hosts: Vec<HostPlan>,
}

impl CatalogPlan {
    #[allow(dead_code)] // consumed by the areas-sync writer
    pub(crate) fn edits(&self) -> Vec<&FileEdit> {
        self.hosts
            .iter()
            .filter_map(|host| host.edit.as_ref())
            .collect()
    }

    #[allow(dead_code)] // consumed by graph/planner tests and areas-sync
    pub(crate) fn host(&self, path: &Path) -> Option<&HostPlan> {
        self.hosts.iter().find(|host| host.path.as_path() == path)
    }
}

/// Plan catalogs for every valid host and cleanup candidate. This function
/// reads only retained snapshot bytes and has no filesystem effects.
pub(crate) fn plan_area_catalogs(
    snapshot: &VaultSnapshot,
    graph: &AreaGraph,
) -> CatalogPlan {
    let root_path = graph.root.as_ref().and_then(|root| root.path.as_ref());
    let mut candidates = BTreeMap::new();
    for (path, file) in &snapshot.files {
        if root_path == Some(path) {
            candidates.insert(path.clone(), HostKind::Root);
            continue;
        }
        match file.lite.kind {
            LiteKind::Area => {
                candidates.insert(path.clone(), HostKind::Area);
            }
            LiteKind::Project { .. } => {
                candidates.insert(path.clone(), HostKind::Project);
            }
            _ => {}
        }
    }

    let mut hosts = Vec::new();
    for (path, kind) in candidates {
        let Some(contents) = snapshot.contents(&path) else {
            continue;
        };
        let (stem, rows): (String, Vec<RowRecord>) = match kind {
            HostKind::Root => {
                let stem = snapshot.files[&path].lite.stem.clone();
                let rows = flatten_rows(graph, &graph.root_children, 0);
                (stem, rows)
            }
            HostKind::Area => {
                let Some(node) = graph.areas.get(&path) else {
                    continue;
                };
                (node.stem.clone(), flatten_rows(graph, &node.children, 0))
            }
            HostKind::Project => {
                (snapshot.files[&path].lite.stem.clone(), Vec::new())
            }
        };
        let callout = render_callout(graph, &rows);
        let planned = host_catalog::plan_host_region(
            path.clone(),
            contents,
            &AREAS,
            &callout,
        );
        let mut state = CatalogState::Current;
        let mut issues = Vec::new();
        let mut edit = None;
        match planned.outcome {
            HostOutcome::Nothing => continue,
            HostOutcome::Current => {}
            HostOutcome::Update {
                state: edit_state, ..
            } => {
                state = match edit_state {
                    EditState::Missing => CatalogState::Missing,
                    EditState::Stale => CatalogState::Stale,
                };
                let path_text = slash(&path);
                issues.push(AreaIssue::warning(
                    &path,
                    "area_catalog_stale",
                    None,
                    format!(
                        "the generated area catalog in {path_text} is {}; run `bob areas sync`",
                        state.as_str()
                    ),
                ));
                edit = planned.edit;
            }
            HostOutcome::Damaged(damage) => {
                state = CatalogState::Damaged;
                issues.push(AreaIssue::error(
                    &path,
                    "area_catalog_damaged",
                    Some(damage.line),
                    format!(
                        "{}: {}; repair the malformed catalog by hand, then run `bob areas sync`",
                        slash(&path),
                        damage.message
                    ),
                ));
            }
        }
        hosts.push(HostPlan {
            path,
            stem,
            kind,
            state,
            rows,
            issues,
            edit,
        });
    }
    CatalogPlan { hosts }
}

fn flatten_rows(
    graph: &AreaGraph,
    roots: &[PathBuf],
    depth: usize,
) -> Vec<RowRecord> {
    fn walk(
        graph: &AreaGraph,
        path: &Path,
        depth: usize,
        seen: &mut std::collections::BTreeSet<PathBuf>,
        rows: &mut Vec<RowRecord>,
    ) {
        if !seen.insert(path.to_path_buf()) {
            return;
        }
        let Some(node) = graph.areas.get(path) else {
            return;
        };
        rows.push(RowRecord {
            path: node.path.clone(),
            link: node.link.clone(),
            depth,
        });
        for child in &node.children {
            walk(graph, child, depth + 1, seen, rows);
        }
    }

    let mut rows = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for root in roots {
        walk(graph, root, depth, &mut seen, &mut rows);
    }
    rows
}

fn render_callout(graph: &AreaGraph, rows: &[RowRecord]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["> [!areas]".to_string(), ">".to_string()];
    for row in rows {
        let indent = "  ".repeat(row.depth);
        let Some(node) = graph.areas.get(&row.path) else {
            continue;
        };
        let link = if row.link == node.stem {
            format!("[[{}]]", node.stem)
        } else {
            format!("[[{}|{}]]", row.link, node.stem)
        };
        let mut line = format!("> {indent}- **{link}**");
        if let Some(summary) = node.summary.as_deref() {
            let summary = inert_text(summary);
            if !summary.is_empty() {
                line.push_str(" — ");
                line.push_str(&summary);
            }
        }
        lines.push(line);
    }
    lines
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
