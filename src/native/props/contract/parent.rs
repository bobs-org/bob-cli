//! Parent checks against the existing note index, area graph, and classifiers.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::{error, Context};
use crate::native::props::{
    document::PropsDocument, roles::NoteKind, InspectContext, PropsError,
};
use crate::native::{
    areas::graph::build_area_graph,
    notebooks::{scan::link_destination, snapshot::LiteKind},
    projects::wikilink_path_target,
    vault_links::{LinkResolution, NoteIndex},
};

fn dependencies(
    context: &InspectContext,
    index: usize,
) -> Result<&Context, PropsError> {
    context
        .contract
        .as_ref()
        .filter(|contract| contract.snapshot.io_errors.is_empty())
        .ok_or_else(|| {
            error(
                "prop_parent_missing",
                "parent",
                index,
                "parent validation needs a complete vault snapshot",
                "Resolve vault read errors, then retry.",
            )
        })
}

fn resolve(
    index: &NoteIndex,
    current: &Path,
    target: &str,
    op: usize,
) -> Result<PathBuf, PropsError> {
    match index.resolve_detailed(Some(current), target) {
        LinkResolution::Found(path) => Ok(path),
        LinkResolution::Missing | LinkResolution::Ambiguous => Err(error(
            "prop_parent_missing", "parent", op,
            format!("parent [[{target}]] is missing or ambiguous"),
            "Choose an existing, unambiguous home; use a path-qualified link when needed.",
        )),
    }
}

fn excluded(path: &Path) -> bool {
    path.components().any(|part| {
        let text = part.as_os_str().to_string_lossy();
        text.eq_ignore_ascii_case("done")
            || text.eq_ignore_ascii_case("_conflicts")
    })
}

// Read decoded scalars so inline comments and quoted keys do not hide an edge.
// Older unquoted [[links]] parse as YAML lists; retain their literal link spelling.
fn parent_target(contents: &str) -> Option<String> {
    let document = PropsDocument::parse(contents);
    let entry = document
        .entries
        .iter()
        .find(|entry| entry.key == "parent")?;
    let value = entry
        .raw_value
        .as_ref()
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or(&document.source[entry.value_range.clone()]);
    wikilink_path_target(value)
}

pub(crate) fn validate(
    context: &InspectContext,
    candidate: &str,
    value: &str,
    op: usize,
) -> Result<String, PropsError> {
    let contract = dependencies(context, op)?;
    let current = Path::new(&context.normalized_path);
    let target = wikilink_path_target(value).ok_or_else(|| {
        error(
            "prop_bad_value",
            "parent",
            op,
            "parent must be one scalar wikilink",
            "Use one quoted link such as \"[[dev]]\".",
        )
    })?;
    if excluded(Path::new(&target)) {
        return Err(error(
            "prop_parent_kind",
            "parent",
            op,
            "archived and conflicted notes cannot be parents",
            "Choose an area or an open project.",
        ));
    }
    // Typed expansion asks for canonicalization before the splice. Give the area
    // graph the proposed edge too, so repairing an existing cycle is permitted.
    let document = PropsDocument::parse(candidate);
    let mut proposed = candidate.to_owned();
    if let Some(entry) =
        document.entries.iter().find(|entry| entry.key == "parent")
    {
        let scalar = serde_yaml::to_string(value).expect("parent scalar");
        proposed.replace_range(entry.value_range.clone(), scalar.trim_end());
    }
    let snapshot = contract
        .snapshot
        .with_overlay([(current.to_owned(), proposed)]);
    let index = snapshot.index();
    let parent = resolve(&index, current, &target, op)?;
    let root = index.resolve(None, "area");
    if parent == current {
        return Err(error(
            "prop_parent_cycle",
            "parent",
            op,
            "a note cannot be its own parent",
            "Choose a home outside this note's descendants.",
        ));
    }
    let kind = snapshot.files.get(&parent).map(|file| file.lite.kind);
    let is_area = context.roles.note_kind != NoteKind::Project;
    let allowed = if is_area {
        root.as_ref() == Some(&parent) || kind == Some(LiteKind::Area)
    } else {
        kind == Some(LiteKind::Area)
            || kind == Some(LiteKind::Project { terminal: false })
    };
    if !allowed
        || excluded(&parent)
        || (!is_area && root.as_ref() == Some(&parent))
    {
        return Err(error(
            "prop_parent_kind",
            "parent",
            op,
            "parent has the wrong kind or is terminal",
            if is_area {
                "Choose the area root or an area."
            } else {
                "Choose an area or a non-terminal project."
            },
        ));
    }
    // Use the area graph's own cycle/depth rules, then walk the combined
    // area/project ancestry so mixed-kind and already-cyclic chains also refuse.
    if is_area {
        let graph = build_area_graph(&snapshot);
        if graph.findings.iter().any(|finding| {
            finding.path == current && finding.code == "area_cycle"
        }) {
            return Err(error(
                "prop_parent_cycle",
                "parent",
                op,
                "parent creates a cycle or exceeds the area depth limit",
                "Choose a home outside this note's descendants.",
            ));
        }
    }
    let mut seen = BTreeSet::from([current.to_owned()]);
    let mut cursor = parent.clone();
    let mut hops = 1;
    loop {
        if !seen.insert(cursor.clone()) || (is_area && hops > 16) {
            return Err(error(
                "prop_parent_cycle",
                "parent",
                op,
                "parent chain cycles or exceeds the area depth limit",
                "Choose an acyclic home outside this note's descendants.",
            ));
        }
        if root.as_ref() == Some(&cursor) {
            break;
        }
        let next = snapshot
            .files
            .get(&cursor)
            .and_then(|file| parent_target(&file.header));
        let Some(next) = next else {
            break;
        };
        cursor = resolve(&index, &cursor, &next, op)?;
        hops += 1;
    }
    Ok(format!("[[{}]]", link_destination(&index, &parent)))
}

pub(crate) fn validate_root(
    context: &InspectContext,
    candidate: &str,
    op: usize,
) -> Result<(), PropsError> {
    let contract = dependencies(context, op)?;
    let parent = parent_target(candidate);
    let index = contract.snapshot.index();
    let parent = parent.and_then(|target| {
        index.resolve(Some(Path::new(&context.normalized_path)), &target)
    });
    if parent
        .as_ref()
        .and_then(|path| contract.snapshot.files.get(path))
        .is_some_and(|file| matches!(file.lite.kind, LiteKind::Project { .. }))
    {
        Ok(())
    } else {
        Err(error("prop_setting_disabled", "root", op, "already a root (top-level project)", "Stats root is available only on a project beneath another project."))
    }
}
