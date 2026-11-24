//! Versioned JSON report for `bob areas list` and `bob areas check`.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use super::{
    catalog::{
        plan_area_catalogs, CatalogPlan, CatalogState, HostKind, RowRecord,
    },
    graph::build_area_graph,
    slash, AreaGraph, AreaIssue, AreaRoot, RootState,
};
use crate::native::{
    notebooks::{
        model::Severity,
        snapshot::{LiteKind, VaultSnapshot},
    },
    plan_budget::today::today_tasks,
    pomodoro,
    pomodoro_homes::{DiskHomeSource, Home, HomeHueSource, HomeResolver, Hue},
    vault_links::LinkResolution,
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rollup {
    pub(crate) direct: usize,
    pub(crate) subtree: usize,
}

impl Rollup {
    fn to_json(self) -> Value {
        json!({ "direct": self.direct, "subtree": self.subtree })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AreaEntry {
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    pub(crate) link: String,
    pub(crate) title: String,
    pub(crate) titled: bool,
    pub(crate) summary: Option<String>,
    pub(crate) parent: Option<PathBuf>,
    pub(crate) parent_raw: Option<String>,
    pub(crate) depth: Option<usize>,
    pub(crate) top: Option<String>,
    pub(crate) hue: Option<Hue>,
    pub(crate) hue_source: Option<HomeHueSource>,
    pub(crate) children: Vec<PathBuf>,
    pub(crate) projects: Rollup,
    pub(crate) today: Rollup,
    pub(crate) issues: Vec<AreaIssue>,
    pub(crate) contract_issues:
        Vec<crate::native::props::contract::issues::ContractIssue>,
}

impl AreaEntry {
    fn to_json(&self) -> Value {
        json!({
            "path": slash(&self.path),
            "stem": self.stem,
            "link": self.link,
            "title": self.title,
            "titled": self.titled,
            "summary": self.summary,
            "parent": self.parent.as_ref().map(|path| slash(path)),
            "parent_raw": self.parent_raw,
            "depth": self.depth,
            "top": self.top,
            "hue": self.hue.map(Hue::name),
            "hue_source": self.hue_source.map(|source| match source {
                HomeHueSource::Color => "color",
                HomeHueSource::Muted => "muted",
                HomeHueSource::Hash => "hash",
            }),
            "children": self.children.iter().map(|path| slash(path)).collect::<Vec<_>>(),
            "projects": self.projects.to_json(),
            "today": self.today.to_json(),
            "issues": self.issues.iter().map(AreaIssue::to_json).collect::<Vec<_>>(),
            "contract_issues": self.contract_issues,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogRecord {
    pub(crate) host: String,
    pub(crate) path: PathBuf,
    pub(crate) kind: HostKind,
    pub(crate) state: CatalogState,
    pub(crate) rows: Vec<RowRecord>,
    pub(crate) issues: Vec<AreaIssue>,
}

impl CatalogRecord {
    fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "path": slash(&self.path),
            "kind": self.kind.as_str(),
            "state": self.state.as_str(),
            "rows": self.rows.iter().map(|row| json!({
                "path": slash(&row.path),
                "link": row.link,
                "depth": row.depth,
            })).collect::<Vec<_>>(),
            "issues": self.issues.iter().map(AreaIssue::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CatalogCounts {
    pub(crate) current: usize,
    pub(crate) missing: usize,
    pub(crate) stale: usize,
    pub(crate) damaged: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AreaCounts {
    pub(crate) areas: usize,
    pub(crate) top_level: usize,
    pub(crate) hosts: usize,
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
    pub(crate) catalogs: CatalogCounts,
}

#[derive(Debug, Clone)]
pub(crate) struct AreaReport {
    pub(crate) root: AreaRoot,
    pub(crate) areas: Vec<AreaEntry>,
    pub(crate) catalogs: Vec<CatalogRecord>,
    pub(crate) findings: Vec<AreaIssue>,
    pub(crate) counts: AreaCounts,
}

impl AreaReport {
    pub(crate) fn ok(&self) -> bool {
        self.counts.errors == 0
    }

    pub(crate) fn to_json(&self, command: &str) -> Value {
        json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.ok(),
            "command": command,
            "root": {
                "path": self.root.path.as_ref().map(|path| slash(path)),
                "stem": self.root.path.as_ref().and_then(|path| {
                    path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string)
                }),
                "state": self.root.state.as_str(),
            },
            "areas": self.areas.iter().map(AreaEntry::to_json).collect::<Vec<_>>(),
            "catalogs": self.catalogs.iter().map(CatalogRecord::to_json).collect::<Vec<_>>(),
            "findings": self.findings.iter().map(AreaIssue::to_json).collect::<Vec<_>>(),
            "counts": {
                "areas": self.counts.areas,
                "top_level": self.counts.top_level,
                "hosts": self.counts.hosts,
                "errors": self.counts.errors,
                "warnings": self.counts.warnings,
                "catalogs": {
                    "current": self.counts.catalogs.current,
                    "missing": self.counts.catalogs.missing,
                    "stale": self.counts.catalogs.stale,
                    "damaged": self.counts.catalogs.damaged,
                },
            },
        })
    }
}

pub(crate) struct ScanOutput {
    pub(crate) report: AreaReport,
    pub(crate) io_errors: Vec<String>,
}

pub(crate) fn scan_areas(bob_dir: &Path) -> ScanOutput {
    let snapshot = VaultSnapshot::load(bob_dir);
    let io_errors = snapshot.io_errors.clone();
    let report = build_report(&snapshot, bob_dir);
    ScanOutput { report, io_errors }
}

pub(crate) fn build_report(
    snapshot: &VaultSnapshot,
    bob_dir: &Path,
) -> AreaReport {
    let graph = build_area_graph(snapshot);
    let plan = plan_area_catalogs(snapshot, &graph);
    let mut resolver = HomeResolver::new(DiskHomeSource::new(bob_dir));
    let project_direct = project_directs(snapshot, &mut resolver);
    let today_direct = today_directs(bob_dir, &mut resolver);
    assemble(
        snapshot,
        &graph,
        &plan,
        &mut resolver,
        &project_direct,
        &today_direct,
    )
}

fn assemble(
    snapshot: &VaultSnapshot,
    graph: &AreaGraph,
    plan: &CatalogPlan,
    resolver: &mut HomeResolver<DiskHomeSource>,
    project_direct: &HashMap<PathBuf, usize>,
    today_direct: &HashMap<PathBuf, usize>,
) -> AreaReport {
    let root = graph.root.clone().unwrap_or(AreaRoot {
        state: RootState::Missing,
        path: None,
    });
    let mut visited = BTreeSet::new();
    let mut areas = Vec::new();
    for path in &graph.root_children {
        push_tree(
            snapshot,
            graph,
            path,
            resolver,
            project_direct,
            today_direct,
            &mut visited,
            &mut areas,
        );
    }
    let mut unplaced: Vec<&super::AreaNode> = graph
        .areas
        .values()
        .filter(|node| !visited.contains(&node.path))
        .collect();
    unplaced.sort_by(|left, right| {
        left.stem
            .to_ascii_lowercase()
            .cmp(&right.stem.to_ascii_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    for node in unplaced {
        areas.push(entry_from(
            snapshot,
            graph,
            node,
            resolver,
            project_direct,
            today_direct,
        ));
    }

    let catalogs: Vec<CatalogRecord> = plan
        .hosts
        .iter()
        .map(|host| CatalogRecord {
            host: host.stem.clone(),
            path: host.path.clone(),
            kind: host.kind,
            state: host.state,
            rows: host.rows.clone(),
            issues: host.issues.clone(),
        })
        .collect();

    let mut findings = graph.findings.clone();
    for catalog in &catalogs {
        findings.extend(catalog.issues.iter().cloned());
    }
    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.code.cmp(right.code))
    });

    let mut catalog_counts = CatalogCounts::default();
    for catalog in &catalogs {
        match catalog.state {
            CatalogState::Current => catalog_counts.current += 1,
            CatalogState::Missing => catalog_counts.missing += 1,
            CatalogState::Stale => catalog_counts.stale += 1,
            CatalogState::Damaged => catalog_counts.damaged += 1,
        }
    }
    let (errors, warnings) = count_severities(&findings);
    let hosts = catalogs
        .iter()
        .filter(|catalog| !catalog.rows.is_empty())
        .count();
    let counts = AreaCounts {
        areas: areas.len(),
        top_level: graph.root_children.len(),
        hosts,
        errors,
        warnings,
        catalogs: catalog_counts,
    };
    AreaReport {
        root,
        areas,
        catalogs,
        findings,
        counts,
    }
}

fn push_tree(
    snapshot: &VaultSnapshot,
    graph: &AreaGraph,
    path: &Path,
    resolver: &mut HomeResolver<DiskHomeSource>,
    project_direct: &HashMap<PathBuf, usize>,
    today_direct: &HashMap<PathBuf, usize>,
    visited: &mut BTreeSet<PathBuf>,
    areas: &mut Vec<AreaEntry>,
) {
    if !visited.insert(path.to_path_buf()) {
        return;
    }
    let Some(node) = graph.areas.get(path) else {
        return;
    };
    areas.push(entry_from(
        snapshot,
        graph,
        node,
        resolver,
        project_direct,
        today_direct,
    ));
    for child in &node.children {
        push_tree(
            snapshot,
            graph,
            child,
            resolver,
            project_direct,
            today_direct,
            visited,
            areas,
        );
    }
}

fn entry_from(
    snapshot: &VaultSnapshot,
    graph: &AreaGraph,
    node: &super::AreaNode,
    resolver: &mut HomeResolver<DiskHomeSource>,
    project_direct: &HashMap<PathBuf, usize>,
    today_direct: &HashMap<PathBuf, usize>,
) -> AreaEntry {
    let home = resolver.resolve_note(&node.path);
    let projects_direct = *project_direct.get(&node.path).unwrap_or(&0);
    let today_direct_count = *today_direct.get(&node.path).unwrap_or(&0);
    AreaEntry {
        path: node.path.clone(),
        stem: node.stem.clone(),
        link: node.link.clone(),
        title: node.title.clone(),
        titled: node.titled,
        summary: node.summary.clone(),
        parent: node.parent.clone(),
        parent_raw: node.parent_raw.clone(),
        depth: node.depth,
        top: node.top.clone(),
        hue: home.hue,
        hue_source: home.hue_source,
        children: node.children.clone(),
        projects: Rollup {
            direct: projects_direct,
            subtree: subtree_count(graph, &node.path, project_direct),
        },
        today: Rollup {
            direct: today_direct_count,
            subtree: subtree_count(graph, &node.path, today_direct),
        },
        issues: node.issues.clone(),
        contract_issues: contract_issues_for(snapshot, node),
    }
}

fn contract_issues_for(
    snapshot: &VaultSnapshot,
    node: &super::AreaNode,
) -> Vec<crate::native::props::contract::issues::ContractIssue> {
    use crate::native::props::contract::issues;
    let mut found = Vec::new();
    if let Some(file) = snapshot.files.get(&node.path) {
        let contents = file.contents.as_deref().unwrap_or(&file.header);
        found.extend(issues::from_contents(contents));
    }
    for issue in &node.issues {
        if let Some(mapped) = issues::from_area_codes(
            issue.code,
            issue.severity.as_str(),
            issue.line,
            &issue.message,
        ) {
            found.push(mapped);
        }
    }
    issues::merge(found)
}

fn subtree_count(
    graph: &AreaGraph,
    path: &Path,
    directs: &HashMap<PathBuf, usize>,
) -> usize {
    let mut total = *directs.get(path).unwrap_or(&0);
    let Some(node) = graph.areas.get(path) else {
        return total;
    };
    for child in &node.children {
        total += subtree_count(graph, child, directs);
    }
    total
}

fn project_directs(
    snapshot: &VaultSnapshot,
    resolver: &mut HomeResolver<DiskHomeSource>,
) -> HashMap<PathBuf, usize> {
    let mut counts = HashMap::new();
    for (path, file) in &snapshot.files {
        match file.lite.kind {
            LiteKind::Project { terminal: false } => {}
            _ => continue,
        }
        let home = resolver.resolve_note(path);
        if let Some(area) = nearest_area(&home) {
            *counts.entry(area).or_insert(0) += 1;
        }
    }
    counts
}

fn today_directs(
    bob_dir: &Path,
    resolver: &mut HomeResolver<DiskHomeSource>,
) -> HashMap<PathBuf, usize> {
    let mut counts = HashMap::new();
    let day_file = pomodoro::day_file_for(bob_dir);
    let absolute = if day_file.is_absolute() {
        day_file.clone()
    } else {
        bob_dir.join(&day_file)
    };
    let Ok(contents) = fs::read_to_string(&absolute) else {
        return counts;
    };
    let daily_relative = absolute
        .strip_prefix(bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_file.clone());
    let daily_display = slash(&daily_relative);
    let tasks = today_tasks(bob_dir, &daily_display, &contents).tasks;
    for task in tasks {
        let home = resolver.home_for_link(
            &daily_relative,
            LinkResolution::Found(PathBuf::from(&task.path)),
            &task.block_id,
        );
        if let Some(area) = nearest_area(&home) {
            *counts.entry(area).or_insert(0) += 1;
        }
    }
    counts
}

fn nearest_area(home: &Home) -> Option<PathBuf> {
    if let Some(subarea) = &home.subarea {
        return Some(PathBuf::from(subarea));
    }
    if home.area.starts_with('@') {
        return None;
    }
    Some(PathBuf::from(&home.area))
}

fn count_severities(findings: &[AreaIssue]) -> (usize, usize) {
    let mut errors = 0;
    let mut warnings = 0;
    for finding in findings {
        match finding.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
        }
    }
    (errors, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::pomodoro_homes::fallback_hue;

    fn write(root: &Path, name: &str, contents: &str) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn area(parent: &str, extra: &str) -> String {
        format!(
            "---\ntype: \"[[area]]\"\nparent: \"[[{parent}]]\"\n{extra}---\n\n# Area\n"
        )
    }

    fn project(parent: &str, status: &str) -> String {
        format!(
            "---\ntype: \"[[project]]\"\nparent: \"[[{parent}]]\"\nstatus: {status}\n---\n\n# Project\n\n- [ ] #task #prj Do it ^prj\n"
        )
    }

    fn report_of(root: &Path) -> AreaReport {
        let snapshot = VaultSnapshot::load(root);
        assert!(snapshot.io_errors.is_empty(), "{:?}", snapshot.io_errors);
        build_report(&snapshot, root)
    }

    fn entry<'a>(report: &'a AreaReport, stem: &str) -> &'a AreaEntry {
        report
            .areas
            .iter()
            .find(|entry| entry.stem == stem)
            .unwrap_or_else(|| panic!("missing area {stem}"))
    }

    #[test]
    fn tree_order_is_depth_first_then_unplaced() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "area.md", "# Area root\n");
        write(root, "cash.md", &area("area", "color: magenta\n"));
        write(root, "bank.md", &area("cash", ""));
        write(root, "body.md", &area("area", ""));
        write(root, "orphan.md", &area("missing", ""));
        let report = report_of(root);
        let stems: Vec<_> = report
            .areas
            .iter()
            .map(|entry| entry.stem.as_str())
            .collect();
        assert_eq!(stems, ["body", "cash", "bank", "orphan"]);
        assert_eq!(entry(&report, "cash").depth, Some(1));
        assert_eq!(entry(&report, "bank").depth, Some(2));
        assert_eq!(entry(&report, "orphan").depth, None);
        assert_eq!(entry(&report, "orphan").top, None);
        assert_eq!(
            entry(&report, "orphan").issues[0].code,
            "area_parent_unresolved"
        );
    }

    #[test]
    fn unplaced_areas_follow_the_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "area.md", "# Area root\n");
        write(root, "zeta.md", &area("gone", ""));
        write(root, "alpha.md", &area("gone", ""));
        write(root, "meet.md", &area("area", ""));
        let report = report_of(root);
        let stems: Vec<_> = report
            .areas
            .iter()
            .map(|entry| entry.stem.as_str())
            .collect();
        assert_eq!(stems, ["meet", "alpha", "zeta"]);
    }

    #[test]
    fn hue_sources_cover_color_muted_hash_and_a_null_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "area.md", "# Area root\n");
        write(root, "cash.md", &area("area", "color: magenta\n"));
        write(root, "job.md", &area("area", "color: grey\n"));
        write(root, "body.md", &area("area", ""));
        let report = report_of(root);
        assert_eq!(report.root.state, RootState::Found);
        assert!(report.areas.iter().all(|entry| entry.stem != "area"));
        let cash = entry(&report, "cash");
        assert_eq!(cash.hue, Some(Hue::Magenta));
        assert_eq!(cash.hue_source, Some(HomeHueSource::Color));
        let job = entry(&report, "job");
        assert_eq!(job.hue, None);
        assert_eq!(job.hue_source, Some(HomeHueSource::Muted));
        let body = entry(&report, "body");
        assert_eq!(body.hue, Some(fallback_hue("body")));
        assert_eq!(body.hue_source, Some(HomeHueSource::Hash));
    }

    #[test]
    fn project_rollup_counts_nearest_area_and_skips_terminal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "area.md", "# Area root\n");
        write(root, "cash.md", &area("area", ""));
        write(root, "bank.md", &area("cash", ""));
        write(root, "direct.md", &project("cash", "wip"));
        write(root, "nested.md", &project("direct", "wip"));
        write(root, "bank_pr.md", &project("bank", "wip"));
        write(root, "done.md", &project("cash", "done"));
        let report = report_of(root);
        let cash = entry(&report, "cash");
        let bank = entry(&report, "bank");
        assert_eq!(cash.projects.direct, 2, "direct + nested under cash");
        assert_eq!(cash.projects.subtree, 3);
        assert_eq!(bank.projects.direct, 1);
        assert_eq!(bank.projects.subtree, 1);
    }

    #[test]
    fn today_rollup_counts_area_project_root_override_and_missing_daily() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "area.md", "# Area root\n");
        write(root, "cash.md", &area("area", ""));
        write(root, "meet.md", &area("area", ""));
        write(root, "spend.md", &project("cash", "wip"));
        write(
            root,
            "cash.md",
            "---\ntype: \"[[area]]\"\nparent: \"[[area]]\"\n---\n\n# Cash\n\n- [ ] #task Pay bills ^bills\n",
        );
        write(
            root,
            "spend.md",
            "---\ntype: \"[[project]]\"\nparent: \"[[cash]]\"\nstatus: wip\n---\n\n# Spend\n\n- [ ] #task #prj Budget ^prj\n- [ ] #task File taxes ^taxes\n",
        );
        write(
            root,
            "meet.md",
            "---\ntype: \"[[area]]\"\nparent: \"[[area]]\"\n---\n\n# Meet\n",
        );

        let missing = report_of(root);
        assert_eq!(entry(&missing, "cash").today.direct, 0);
        assert_eq!(entry(&missing, "meet").today.direct, 0);

        let daily = "2026/20261115.md";
        write(
            root,
            daily,
            "# 2026-11-15\n\
             \n\
             ## Pomodoros\n\
             \n\
             - [ ] () — WORK\n\
             \x20   - 🍅 [[cash#^bills]]\n\
             \x20   - 🍅 [[spend#^taxes]]\n\
             \x20   - 🍅 [[#^standup]]\n\
             \n\
             ## Events\n\
             \n\
             - [ ] #task Standup [root:: meet] ^standup\n",
        );
        let report =
            crate::native::env::with_var("BOB_DAY_FILE", daily, || {
                report_of(root)
            });
        let cash = entry(&report, "cash");
        let meet = entry(&report, "meet");
        assert_eq!(cash.today.direct, 2, "area-resident plus project-resident");
        assert_eq!(cash.today.subtree, 2);
        assert_eq!(meet.today.direct, 1, "daily Event Task rooted at meet");
        assert_eq!(meet.today.subtree, 1);
    }
}
