//! Validated area-parent graph, computed only from a vault snapshot.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use serde_yaml::Value;

use super::{AreaGraph, AreaIssue, AreaNode, AreaRoot, RootState};
use crate::native::{
    notebooks::{
        scan,
        snapshot::{LiteKind, VaultSnapshot},
    },
    projects::wikilink_path_target,
    vault_links::{LinkResolution, NoteIndex},
};

#[derive(Debug, Clone)]
struct ParentEdge {
    parent: Option<PathBuf>,
    line: Option<usize>,
    unquoted: bool,
}

/// Build the root resolution, validated parent edges, findings, and rooted
/// tree fields for one immutable vault snapshot.
pub(crate) fn build_area_graph(snapshot: &VaultSnapshot) -> AreaGraph {
    let index = snapshot.index();
    let root_resolution = index.resolve_detailed(None, "area");
    let (root, mut findings) = match root_resolution {
        LinkResolution::Found(path) => (
            AreaRoot {
                state: RootState::Found,
                path: Some(path),
            },
            Vec::new(),
        ),
        LinkResolution::Missing => (
            AreaRoot {
                state: RootState::Missing,
                path: None,
            },
            vec![AreaIssue::warning(
                "area.md",
                "area_root_missing",
                None,
                "the bare `[[area]]` link does not resolve to a root note",
            )],
        ),
        LinkResolution::Ambiguous => (
            AreaRoot {
                state: RootState::Ambiguous,
                path: None,
            },
            vec![AreaIssue::warning(
                "area.md",
                "area_root_ambiguous",
                None,
                "the bare `[[area]]` link resolves to several notes; add an exact `area.md` root",
            )],
        ),
    };
    let root_path = root.path.clone();
    let mut root_children = Vec::new();
    let root_finding_fired = root.state != RootState::Found;

    let mut nodes = BTreeMap::new();
    let mut edges = BTreeMap::new();
    for (path, file) in &snapshot.files {
        // Every possible `[[area]]` target is a root candidate, never a
        // regular area: the resolved root is a terminal sentinel, and its
        // unresolved namesakes stay out of the graph so one root finding
        // covers them instead of per-area parent noise.
        if file.lite.kind != LiteKind::Area
            || root_path.as_ref() == Some(path)
            || file.lite.stem.eq_ignore_ascii_case("area")
        {
            continue;
        }
        let contents = snapshot.contents(path).unwrap_or("");
        let (title, titled) = scan::derive_title_and_titled(
            contents,
            file.lite.aliases_first.as_deref(),
            &file.lite.stem,
        );
        let summary = yaml_scalar(&file.header, "summary")
            .or_else(|| file.lite.summary_raw.as_deref().map(decode_scalar))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let mut node = AreaNode {
            path: path.clone(),
            stem: file.lite.stem.clone(),
            link: scan::link_destination(&index, path),
            title,
            titled,
            summary,
            parent: None,
            parent_raw: file.lite.parent_raw.clone(),
            depth: None,
            top: None,
            children: Vec::new(),
            issues: Vec::new(),
        };
        add_summary_findings(path, node.summary.as_deref(), &mut node.issues);

        let edge = resolve_edge(
            path,
            &file.header,
            &file.lite.parent_raw,
            file.lite.parent_line,
            file.lite.parent_count,
            &index,
            &snapshot.files,
            root_path.as_deref(),
            root_finding_fired,
            &mut node.issues,
        );
        node.parent = edge.parent.clone();
        if edge.unquoted {
            node.issues.push(AreaIssue::warning(
                path,
                "area_parent_unquoted",
                edge.line,
                "quote the parent as `\"[[stem]]\"`; bare `[[stem]]` parses as a nested list",
            ));
        }
        edges.insert(path.clone(), edge);
        nodes.insert(path.clone(), node);
    }

    // Find every cycle before applying the hop cap, so even a long cycle has
    // all of its member edges removed rather than looking like a long chain.
    let cycle_members = cycle_members(&edges, &nodes, root_path.as_deref());
    for path in &cycle_members {
        if let Some(node) = nodes.get_mut(path) {
            let line = edges.get(path).and_then(|edge| edge.line);
            node.issues.push(AreaIssue::error(
                path,
                "area_cycle",
                line,
                "area parent chain cycles; break one of the cycle links",
            ));
            node.parent = None;
        }
        if let Some(edge) = edges.get_mut(path) {
            edge.parent = None;
        }
    }

    // A rooted chain is limited to sixteen parent hops, counting its final
    // edge to the root. Mark the area whose own walk exceeds that bound and
    // cut that edge; other valid descendants can still host their children.
    let mut too_deep = Vec::new();
    for start in nodes.keys() {
        let mut current = start.clone();
        let mut hops = 0;
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.clone()) {
                break;
            }
            let Some(parent) =
                edges.get(&current).and_then(|edge| edge.parent.as_ref())
            else {
                break;
            };
            hops += 1;
            if hops > 16 {
                too_deep.push(start.clone());
                break;
            }
            if root_path.as_ref() == Some(parent) {
                break;
            }
            current = parent.clone();
        }
    }
    for path in too_deep {
        if let Some(node) = nodes.get_mut(&path) {
            let line = edges.get(&path).and_then(|edge| edge.line);
            node.issues.push(AreaIssue::error(
                &path,
                "area_cycle",
                line,
                "area parent chain exceeds the 16-hop limit",
            ));
            node.parent = None;
        }
        if let Some(edge) = edges.get_mut(&path) {
            edge.parent = None;
        }
    }

    // The edge map is now acyclic and bounded. Materialize its child lists
    // and calculate depth/top only for areas that reach the root sentinel.
    for (child, edge) in &edges {
        if let Some(parent) = &edge.parent
            && let Some(parent_node) = nodes.get_mut(parent)
        {
            parent_node.children.push(child.clone());
        }
    }
    let area_paths = nodes.keys().cloned().collect::<Vec<_>>();
    for path in area_paths {
        let mut children =
            std::mem::take(&mut nodes.get_mut(&path).unwrap().children);
        sort_paths(&mut children, &nodes);
        nodes.get_mut(&path).unwrap().children = children;
    }
    if let Some(root_path) = &root_path {
        let mut top_level = nodes
            .iter()
            .filter_map(|(path, _node)| {
                (edges.get(path).and_then(|edge| edge.parent.as_ref())
                    == Some(root_path))
                .then_some(path.clone())
            })
            .collect::<Vec<_>>();
        sort_paths(&mut top_level, &nodes);
        root_children = top_level;
        let starts = nodes.keys().cloned().collect::<Vec<_>>();
        for start in starts {
            let mut chain = Vec::new();
            let mut current = start.clone();
            let mut seen = BTreeSet::new();
            let mut reaches_root = false;
            loop {
                if !seen.insert(current.clone()) {
                    break;
                }
                chain.push(current.clone());
                let Some(parent) =
                    edges.get(&current).and_then(|edge| edge.parent.as_ref())
                else {
                    break;
                };
                if parent == root_path {
                    reaches_root = true;
                    break;
                }
                current = parent.clone();
            }
            if reaches_root {
                let top = chain
                    .last()
                    .and_then(|path| nodes.get(path))
                    .map(|node| node.stem.clone());
                if let Some(node) = nodes.get_mut(&start) {
                    node.depth = Some(chain.len());
                    node.top = top;
                }
            }
        }
    }

    for node in nodes.values_mut() {
        node.issues.sort_by(|left, right| {
            left.line
                .cmp(&right.line)
                .then_with(|| left.code.cmp(right.code))
        });
        findings.extend(node.issues.iter().cloned());
    }
    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.code.cmp(right.code))
    });

    AreaGraph {
        root: Some(root),
        areas: nodes,
        root_children,
        findings,
    }
}

fn resolve_edge(
    current: &Path,
    header: &str,
    raw: &Option<String>,
    parent_line: Option<usize>,
    parent_count: usize,
    index: &NoteIndex,
    files: &BTreeMap<PathBuf, crate::native::notebooks::snapshot::SnapshotFile>,
    root_path: Option<&Path>,
    root_finding_fired: bool,
    issues: &mut Vec<AreaIssue>,
) -> ParentEdge {
    let raw = raw.as_deref();
    let line = parent_line;
    if parent_count > 1
        || (yaml_parent_is_list(header)
            && !raw.is_some_and(is_unquoted_wikilink))
    {
        issues.push(AreaIssue::error(
            current,
            "area_parent_list",
            line,
            "area needs exactly one scalar `parent` link",
        ));
        return ParentEdge {
            parent: None,
            line,
            unquoted: false,
        };
    }
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        issues.push(AreaIssue::error(
            current,
            "area_parent_missing",
            line,
            "area needs one `parent` link to the root or another area",
        ));
        return ParentEdge {
            parent: None,
            line,
            unquoted: false,
        };
    };
    let unquoted = is_unquoted_wikilink(raw);
    let Some(target) = wikilink_path_target(raw) else {
        issues.push(AreaIssue::error(
            current,
            "area_parent_unresolved",
            line,
            "parent does not resolve to the root note or an area note",
        ));
        return ParentEdge {
            parent: None,
            line,
            unquoted: false,
        };
    };
    if root_finding_fired
        && target
            .strip_suffix(".md")
            .unwrap_or(&target)
            .eq_ignore_ascii_case("area")
    {
        return ParentEdge {
            parent: None,
            line,
            unquoted: false,
        };
    }
    match index.resolve_detailed(Some(current), &target) {
        LinkResolution::Missing => {
            issues.push(AreaIssue::error(
                current,
                "area_parent_unresolved",
                line,
                format!("parent `[[{target}]]` does not resolve to a note"),
            ));
            ParentEdge {
                parent: None,
                line,
                unquoted: false,
            }
        }
        LinkResolution::Ambiguous => {
            issues.push(AreaIssue::error(
                current,
                "area_parent_ambiguous",
                line,
                "parent link is ambiguous; use a path-qualified link",
            ));
            ParentEdge {
                parent: None,
                line,
                unquoted: false,
            }
        }
        LinkResolution::Found(parent) => {
            if parent == current {
                issues.push(AreaIssue::error(
                    current,
                    "area_parent_self",
                    line,
                    "an area cannot be its own parent",
                ));
                return ParentEdge {
                    parent: None,
                    line,
                    unquoted,
                };
            }
            if root_path == Some(parent.as_path()) {
                return ParentEdge {
                    parent: Some(parent),
                    line,
                    unquoted,
                };
            }
            if files
                .get(&parent)
                .is_some_and(|file| file.lite.kind == LiteKind::Area)
            {
                return ParentEdge {
                    parent: Some(parent),
                    line,
                    unquoted,
                };
            }
            issues.push(AreaIssue::error(
                current,
                "area_parent_kind",
                line,
                "parent is neither the `[[area]]` root note nor an area note",
            ));
            ParentEdge {
                parent: None,
                line,
                unquoted,
            }
        }
    }
}

fn cycle_members(
    edges: &BTreeMap<PathBuf, ParentEdge>,
    nodes: &BTreeMap<PathBuf, AreaNode>,
    root_path: Option<&Path>,
) -> BTreeSet<PathBuf> {
    let mut done = BTreeSet::new();
    let mut cycles = BTreeSet::new();
    for start in nodes.keys() {
        if done.contains(start) {
            continue;
        }
        let mut chain = Vec::new();
        let mut positions = HashMap::new();
        let mut current = start.clone();
        loop {
            if let Some(at) = positions.get(&current).copied() {
                cycles.extend(chain[at..].iter().cloned());
                break;
            }
            if done.contains(&current) {
                break;
            }
            positions.insert(current.clone(), chain.len());
            chain.push(current.clone());
            let Some(parent) =
                edges.get(&current).and_then(|edge| edge.parent.as_ref())
            else {
                break;
            };
            if root_path == Some(parent.as_path())
                || !nodes.contains_key(parent)
            {
                break;
            }
            current = parent.clone();
        }
        done.extend(chain);
    }
    cycles
}

fn sort_paths(paths: &mut [PathBuf], nodes: &BTreeMap<PathBuf, AreaNode>) {
    paths.sort_by(|left, right| {
        let left_node = &nodes[left];
        let right_node = &nodes[right];
        left_node
            .stem
            .to_lowercase()
            .cmp(&right_node.stem.to_lowercase())
            .then_with(|| left.cmp(right))
    });
}

fn yaml_parent_is_list(header: &str) -> bool {
    yaml_scalar_value(header, "parent").is_some_and(|value| value.is_sequence())
}

fn yaml_scalar(header: &str, key: &str) -> Option<String> {
    yaml_scalar_value(header, key).and_then(|value| match value {
        Value::String(text) => Some(text),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    })
}

fn yaml_scalar_value(header: &str, key: &str) -> Option<Value> {
    let block = header.strip_prefix("---\n")?.rsplit_once("\n---")?.0;
    serde_yaml::from_str::<Value>(block)
        .ok()?
        .as_mapping()?
        .get(Value::String(key.to_string()))
        .cloned()
}

fn decode_scalar(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with(['"', '\''])
        && let Ok(Value::String(text)) = serde_yaml::from_str::<Value>(trimmed)
    {
        return text;
    }
    match trimmed.find(" #") {
        Some(at) => trimmed[..at].trim_end().to_string(),
        None => trimmed.to_string(),
    }
}

fn is_unquoted_wikilink(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.starts_with("[[") && trimmed.ends_with("]]")
}

fn add_summary_findings(
    path: &Path,
    summary: Option<&str>,
    issues: &mut Vec<AreaIssue>,
) {
    let Some(summary) = summary else {
        return;
    };
    if summary.contains('\n') {
        issues.push(AreaIssue::warning(
            path,
            "area_summary_multiline",
            None,
            "keep `summary:` to one line",
        ));
    }
    if summary.lines().next().unwrap_or(summary).chars().count() > 140 {
        issues.push(AreaIssue::warning(
            path,
            "area_summary_long",
            None,
            "keep `summary:` to at most 140 characters",
        ));
    }
}
