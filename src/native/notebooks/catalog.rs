//! The pure catalog planner: from one analyzed vault graph to a
//! per-host plan of generated `## Notebooks` blocks.
//!
//! Nothing here reads the filesystem, takes a lock, writes, commits, or
//! prints. A caller (`sync`, `create`, later capture) feeds it a graph built
//! from a [`super::snapshot::VaultSnapshot`], possibly with staged files
//! overlaid, and receives host states, findings, and before/after bytes.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::{
    model::{CatalogRecord, CatalogState, NotebookIssue},
    region::{EditState, HostOutcome},
};
use crate::native::host_catalog::{self, NOTEBOOKS};

pub(crate) use crate::native::host_catalog::FileEdit;

/// An area or project note that carries (or should carry) a catalog.
#[derive(Debug, Clone)]
pub(crate) struct CatalogHost {
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    /// Wikilink destination that resolves to this note.
    pub(crate) link: String,
    /// The host's computed root stem.
    pub(crate) root: Option<String>,
    /// Original bytes, the stale-input preimage for any edit.
    pub(crate) contents: String,
}

/// A structurally valid notebook: it resolves an owner and a root, so it
/// belongs in at least one catalog.
#[derive(Debug, Clone)]
pub(crate) struct CatalogNotebook {
    pub(crate) path: PathBuf,
    pub(crate) link: String,
    pub(crate) title: String,
    pub(crate) summary: Option<String>,
    /// The nearest area or project on the parent chain.
    pub(crate) owner: PathBuf,
    /// The resolved parent note (a host or another notebook).
    pub(crate) parent: PathBuf,
    pub(crate) parent_is_notebook: bool,
    /// Hosts listing this notebook: the owner up to and including its root.
    pub(crate) hosts: Vec<PathBuf>,
    /// Every note on the residence chain, owner and root ancestry included.
    pub(crate) chain: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogGraph {
    pub(crate) hosts: BTreeMap<PathBuf, CatalogHost>,
    pub(crate) notebooks: BTreeMap<PathBuf, CatalogNotebook>,
}

#[derive(Debug, Clone)]
pub(crate) struct HostPlan {
    pub(crate) record: CatalogRecord,
    /// Present for `missing` and `stale` hosts.
    pub(crate) edit: Option<FileEdit>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogPlan {
    pub(crate) hosts: Vec<HostPlan>,
}

impl CatalogPlan {
    pub(crate) fn records(&self) -> Vec<CatalogRecord> {
        self.hosts.iter().map(|host| host.record.clone()).collect()
    }

    pub(crate) fn edits(&self) -> Vec<&FileEdit> {
        self.hosts
            .iter()
            .filter_map(|host| host.edit.as_ref())
            .collect()
    }

    pub(crate) fn host(&self, path: &Path) -> Option<&HostPlan> {
        self.hosts
            .iter()
            .find(|host| Path::new(&host.record.path) == path)
    }
}

/// Plan every host that has expected rows or an existing managed region.
pub(crate) fn plan_catalogs(graph: &CatalogGraph) -> CatalogPlan {
    let mut hosts = Vec::new();
    for host in graph.hosts.values() {
        let members: Vec<&CatalogNotebook> = graph
            .notebooks
            .values()
            .filter(|notebook| notebook.hosts.contains(&host.path))
            .collect();
        let rows = render_rows(graph, host, &members);
        let callout = if rows.is_empty() {
            Vec::new()
        } else {
            let mut lines = vec!["> [!notebooks]".to_string(), ">".to_string()];
            lines.extend(rows);
            lines
        };
        let planned = host_catalog::plan_host_region(
            host.path.clone(),
            &host.contents,
            &NOTEBOOKS,
            &callout,
        );
        let path = slash(&host.path);
        let mut record = CatalogRecord {
            host: host.stem.clone(),
            path: path.clone(),
            root: host.root.clone(),
            state: CatalogState::Current,
            notebooks: members.len(),
            issues: Vec::new(),
        };
        let mut edit = None;
        match planned.outcome {
            HostOutcome::Nothing => continue,
            HostOutcome::Current => {}
            HostOutcome::Update { state, after: _ } => {
                record.state = match state {
                    EditState::Missing => CatalogState::Missing,
                    EditState::Stale => CatalogState::Stale,
                };
                record.issues.push(NotebookIssue::warning(
                    "notebook_catalog_stale",
                    None,
                    format!(
                        "the generated notebook catalog in {path} is {}; run `bob notebooks sync`",
                        record.state.as_str()
                    ),
                ));
                edit = planned.edit;
            }
            HostOutcome::Damaged(damage) => {
                record.state = CatalogState::Damaged;
                record.issues.push(NotebookIssue::error(
                    "notebook_catalog_damaged",
                    Some(damage.line),
                    format!("{path}: {}; repair the old markers by hand, then run `bob notebooks sync`", damage.message),
                ));
            }
        }
        hosts.push(HostPlan { record, edit });
    }
    CatalogPlan { hosts }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Callout list rows for one host: its directly owned forest first, then
/// one `🧩` group per descendant owner, ordered by stem.
fn render_rows(
    graph: &CatalogGraph,
    host: &CatalogHost,
    members: &[&CatalogNotebook],
) -> Vec<String> {
    let mut rows = Vec::new();
    let direct: Vec<&CatalogNotebook> = members
        .iter()
        .copied()
        .filter(|notebook| notebook.owner == host.path)
        .collect();
    push_forest(&mut rows, &direct, 0);

    let mut owners: BTreeSet<(String, PathBuf)> = BTreeSet::new();
    for notebook in members {
        if notebook.owner != host.path
            && let Some(owner) = graph.hosts.get(&notebook.owner)
        {
            owners.insert((owner.stem.to_lowercase(), owner.path.clone()));
        }
    }
    for (_, owner_path) in owners {
        let Some(owner) = graph.hosts.get(&owner_path) else {
            continue;
        };
        let group: Vec<&CatalogNotebook> = members
            .iter()
            .copied()
            .filter(|notebook| notebook.owner == owner_path)
            .collect();
        rows.push(format!("> - 🧩 {}", group_link(owner)));
        push_forest(&mut rows, &group, 1);
    }
    rows
}

fn group_link(owner: &CatalogHost) -> String {
    if owner.link == owner.stem {
        format!("[[{}]]", owner.stem)
    } else {
        format!("[[{}|{}]]", owner.link, inert_text(&owner.stem))
    }
}

/// Depth-first rows for one owner's notebooks, nested by notebook parent.
fn push_forest(
    rows: &mut Vec<String>,
    notebooks: &[&CatalogNotebook],
    base_depth: usize,
) {
    let present: BTreeSet<&Path> = notebooks
        .iter()
        .map(|notebook| notebook.path.as_path())
        .collect();
    let mut children: BTreeMap<&Path, Vec<&CatalogNotebook>> = BTreeMap::new();
    let mut roots: Vec<&CatalogNotebook> = Vec::new();
    for notebook in notebooks {
        if notebook.parent_is_notebook
            && present.contains(notebook.parent.as_path())
        {
            children
                .entry(notebook.parent.as_path())
                .or_default()
                .push(notebook);
        } else {
            roots.push(notebook);
        }
    }
    let mut visited: BTreeSet<&Path> = BTreeSet::new();
    sort_level(&mut roots);
    for root in roots {
        walk(rows, root, base_depth, &children, &mut visited);
    }
}

fn sort_level(level: &mut [&CatalogNotebook]) {
    level.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
}

fn walk<'a>(
    rows: &mut Vec<String>,
    notebook: &'a CatalogNotebook,
    depth: usize,
    children: &BTreeMap<&Path, Vec<&'a CatalogNotebook>>,
    visited: &mut BTreeSet<&'a Path>,
) {
    if !visited.insert(notebook.path.as_path()) {
        return;
    }
    rows.push(notebook_row(notebook, depth));
    let Some(kids) = children.get(notebook.path.as_path()) else {
        return;
    };
    let mut kids = kids.clone();
    sort_level(&mut kids);
    for kid in kids {
        walk(rows, kid, depth + 1, children, visited);
    }
}

fn notebook_row(notebook: &CatalogNotebook, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let mut row = format!(
        "> {indent}- **[[{}|{}]]**",
        notebook.link,
        inert_text(&notebook.title)
    );
    if let Some(summary) = &notebook.summary {
        let summary = inert_text(summary);
        if !summary.is_empty() {
            row.push_str(" — ");
            row.push_str(&summary);
        }
    }
    row
}

/// One line of authored text made inert: whitespace collapsed, and every
/// character that could open a link, wikilink, emphasis, code, HTML, tag,
/// block ID, math, comment, highlight, footnote, or inline field escaped.
pub(crate) fn inert_text(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = collapsed.chars().collect();
    let mut out = String::with_capacity(collapsed.len() + 8);
    for (index, ch) in chars.iter().copied().enumerate() {
        let previous = index.checked_sub(1).map(|at| chars[at]);
        let next = chars.get(index + 1).copied();
        let escape = match ch {
            '\\' | '`' | '*' | '[' | ']' | '<' | '>' | '|' | '^' | '$'
            | '~' => true,
            '_' => {
                !(previous.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric))
            }
            '#' => {
                previous.is_none_or(char::is_whitespace)
                    && next.is_some_and(|next| !next.is_whitespace())
            }
            '=' | '%' => previous == Some(ch) || next == Some(ch),
            ':' => previous == Some(':'),
            '&' => entity_follows(&chars[index + 1..]),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn entity_follows(rest: &[char]) -> bool {
    let run = rest
        .iter()
        .take_while(|ch| ch.is_ascii_alphanumeric() || **ch == '#')
        .count();
    run > 0 && rest.get(run) == Some(&';')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(path: &str, root: &str, body: &str) -> CatalogHost {
        let stem = Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap()
            .to_string();
        CatalogHost {
            path: PathBuf::from(path),
            link: stem.clone(),
            stem,
            root: Some(root.to_string()),
            contents: body.to_string(),
        }
    }

    fn notebook(
        stem: &str,
        title: &str,
        summary: Option<&str>,
        owner: &str,
        parent: &str,
        parent_is_notebook: bool,
        hosts: &[&str],
    ) -> CatalogNotebook {
        CatalogNotebook {
            path: PathBuf::from(format!("{stem}.md")),
            link: stem.to_string(),
            title: title.to_string(),
            summary: summary.map(str::to_string),
            owner: PathBuf::from(owner),
            parent: PathBuf::from(parent),
            parent_is_notebook,
            hosts: hosts.iter().map(PathBuf::from).collect(),
            chain: Vec::new(),
        }
    }

    fn graph(
        hosts: Vec<CatalogHost>,
        notebooks: Vec<CatalogNotebook>,
    ) -> CatalogGraph {
        CatalogGraph {
            hosts: hosts
                .into_iter()
                .map(|host| (host.path.clone(), host))
                .collect(),
            notebooks: notebooks
                .into_iter()
                .map(|notebook| (notebook.path.clone(), notebook))
                .collect(),
        }
    }

    const FRONT: &str = "---\ntype: \"[[area]]\"\n---\n\n# Host\n";

    #[test]
    fn rows_nest_sort_and_group_by_owner() {
        let graph = graph(
            vec![host("dev.md", "dev", FRONT), host("proj.md", "dev", FRONT)],
            vec![
                notebook(
                    "zed",
                    "zed notes",
                    None,
                    "dev.md",
                    "dev.md",
                    false,
                    &["dev.md"],
                ),
                notebook(
                    "dev_tools",
                    "Developer tools",
                    Some("Command-line tools."),
                    "dev.md",
                    "dev.md",
                    false,
                    &["dev.md"],
                ),
                notebook(
                    "git",
                    "Git",
                    Some("Git tips."),
                    "dev.md",
                    "dev_tools.md",
                    true,
                    &["dev.md"],
                ),
                notebook(
                    "p_log",
                    "Project log",
                    None,
                    "proj.md",
                    "proj.md",
                    false,
                    &["proj.md", "dev.md"],
                ),
            ],
        );
        let plan = plan_catalogs(&graph);
        let dev = plan.host(Path::new("dev.md")).expect("dev planned");
        assert_eq!(dev.record.notebooks, 4);
        assert_eq!(dev.record.state, CatalogState::Missing);
        let after = &dev.edit.as_ref().unwrap().after;
        let expected = "\
> - **[[dev_tools|Developer tools]]** — Command-line tools.
>   - **[[git|Git]]** — Git tips.
> - **[[zed|zed notes]]**
> - 🧩 [[proj]]
>   - **[[p_log|Project log]]**";
        assert!(after.contains(expected), "{after}");
        let proj = plan.host(Path::new("proj.md")).expect("proj planned");
        assert_eq!(proj.record.notebooks, 1);
    }

    #[test]
    fn hosts_without_rows_or_region_are_not_planned() {
        let graph = graph(vec![host("dev.md", "dev", FRONT)], vec![]);
        assert!(plan_catalogs(&graph).hosts.is_empty());
    }

    #[test]
    fn empty_stale_host_is_removed_and_reported_stale() {
        let body = format!(
            "{FRONT}\n{}",
            "## Notebooks\n\n> [!notebooks]\n>\n> - old\n"
        );
        let graph = graph(vec![host("dev.md", "dev", &body)], vec![]);
        let plan = plan_catalogs(&graph);
        let dev = &plan.hosts[0];
        assert_eq!(dev.record.state, CatalogState::Stale);
        assert_eq!(dev.record.notebooks, 0);
        assert_eq!(dev.edit.as_ref().unwrap().after, FRONT);
        assert_eq!(dev.record.issues[0].code, "notebook_catalog_stale");
    }

    #[test]
    fn legacy_marked_host_migrates_to_bare_callout() {
        let body = format!(
            "{FRONT}\n{}",
            "## Notebooks\n\n<!-- bob:notebooks:begin v1 -->\n\n> [!notebooks]\n>\n> - old\n\n<!-- bob:notebooks:end -->\n"
        );
        let graph = graph(
            vec![host("dev.md", "dev", &body)],
            vec![notebook(
                "git",
                "Git",
                None,
                "dev.md",
                "dev.md",
                false,
                &["dev.md"],
            )],
        );
        let plan = plan_catalogs(&graph);
        let dev = &plan.hosts[0];
        assert_eq!(dev.record.state, CatalogState::Stale);
        let after = &dev.edit.as_ref().unwrap().after;
        assert!(after.contains("> [!notebooks]"));
        assert!(!after.contains("bob:notebooks:"));
    }

    #[test]
    fn damaged_host_reports_error_without_edit() {
        let body = format!("{FRONT}\n<!-- bob:notebooks:begin v1 -->\n");
        let graph = graph(
            vec![host("dev.md", "dev", &body)],
            vec![notebook(
                "git",
                "Git",
                None,
                "dev.md",
                "dev.md",
                false,
                &["dev.md"],
            )],
        );
        let plan = plan_catalogs(&graph);
        let dev = &plan.hosts[0];
        assert_eq!(dev.record.state, CatalogState::Damaged);
        assert!(dev.edit.is_none());
        assert_eq!(dev.record.issues[0].code, "notebook_catalog_damaged");
        assert_eq!(dev.record.issues[0].line, Some(7));
    }

    #[test]
    fn inert_text_neutralizes_active_markup() {
        assert_eq!(
            inert_text("a *b* `c` [d](e) [[f]] <b>"),
            "a \\*b\\* \\`c\\` \\[d\\](e) \\[\\[f\\]\\] \\<b\\>"
        );
        assert_eq!(inert_text("#tag done ^id"), "\\#tag done \\^id");
        assert_eq!(inert_text("snake_case _x_"), "snake_case \\_x\\_");
        assert_eq!(
            inert_text("a ==b== %%c%% key:: v"),
            "a \\=\\=b\\=\\= \\%\\%c\\%\\% key:\\: v"
        );
        assert_eq!(inert_text("R&D &amp; $5"), "R&D \\&amp; \\$5");
        assert_eq!(inert_text("multi\n  line   text"), "multi line text");
    }
}
