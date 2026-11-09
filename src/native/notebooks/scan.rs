//! One sorted vault walk serving notebook list and check.
//!
//! A single recursive discovery pass indexes relative paths once, reads
//! frontmatter for every included note (refs and record types included,
//! because child-kind validation needs them), and retains full bodies
//! only for notebooks. Ownership reuses the existing `HomeResolver`
//! through `IndexedHomeSource` so root, owner, and hue agree with Stats.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::model::{
    NotebookEntry, NotebookIssue, NotebookReport, NotebookSection,
};
use super::rules;
use crate::native::{
    env as bob_env,
    highlights_ref::humanize_stem,
    pomodoro_homes::{HomeResolver, IndexedHomeSource},
    projects::{
        frontmatter_is_area, frontmatter_is_notebook, frontmatter_is_project,
        frontmatter_value, is_excluded_directory, is_markdown_file,
        parse_frontmatter, wikilink_path_target,
    },
    vault_links::NoteIndex,
};

#[derive(Debug)]
pub(crate) struct ScanOutput {
    pub(crate) report: NotebookReport,
    pub(crate) io_errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct NoteFactsLite {
    relative: PathBuf,
    relative_slash: String,
    stem: String,
    kind: LiteKind,
    parent_raw: Option<String>,
    parent_line: Option<usize>,
    parent_count: usize,
    summary_raw: Option<String>,
    aliases_first: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteKind {
    Area,
    Project { terminal: bool },
    Notebook,
    Ref,
    RecordInstance,
    Other,
    Untyped,
}

pub(crate) fn scan_notebooks(bob_dir: &Path) -> ScanOutput {
    let mut relative_paths: Vec<PathBuf> = Vec::new();
    let mut io_errors = Vec::new();
    collect_markdown(bob_dir, bob_dir, &mut relative_paths, &mut io_errors);
    relative_paths.sort();
    relative_paths.dedup();

    let index = NoteIndex::from_paths(relative_paths.clone());

    // Load record config once for classification.
    let record_ids: BTreeSet<String> =
        match crate::native::config::records::load_records_config() {
            Ok(config) => config.types.into_iter().map(|t| t.id).collect(),
            Err(_) => BTreeSet::new(),
        };

    // Read every included note once.
    let mut lites: BTreeMap<PathBuf, NoteFactsLite> = BTreeMap::new();
    let mut bodies: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut buffers: Vec<(String, String)> = Vec::new();
    for relative in &relative_paths {
        let full = bob_dir.join(relative);
        let contents = match fs::read_to_string(&full) {
            Ok(contents) => contents,
            Err(error) => {
                io_errors.push(format!(
                    "{}: failed to read file: {error}",
                    display_slash(relative)
                ));
                continue;
            }
        };
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            // No frontmatter: untyped note without facts. Still indexed.
            let slash = display_slash(relative);
            let stem = stem_of(relative);
            lites.insert(
                relative.clone(),
                NoteFactsLite {
                    relative: relative.clone(),
                    relative_slash: slash.clone(),
                    stem,
                    kind: LiteKind::Untyped,
                    parent_raw: None,
                    parent_line: None,
                    parent_count: 0,
                    summary_raw: None,
                    aliases_first: None,
                },
            );
            // Header buffer for resolver: empty frontmatter still parses as Other.
            buffers.push((slash, "---\n---\n".to_string()));
            continue;
        };
        let kind = classify_kind(&frontmatter, &record_ids);
        let (parent_raw, parent_line, parent_count) = parent_info(&frontmatter);
        let summary_raw =
            frontmatter_value(&frontmatter, "summary").map(str::to_string);
        let aliases_first = first_alias(&contents);
        let slash = display_slash(relative);
        let stem = stem_of(relative);
        let is_notebook = frontmatter_is_notebook(&frontmatter);
        if is_notebook {
            bodies.insert(relative.clone(), contents.clone());
        }
        // Header buffer: reconstruct minimal frontmatter so NoteFacts parses.
        let header = reconstruct_header(&frontmatter);
        buffers.push((slash.clone(), header));
        lites.insert(
            relative.clone(),
            NoteFactsLite {
                relative: relative.clone(),
                relative_slash: slash,
                stem,
                kind: if is_notebook {
                    LiteKind::Notebook
                } else {
                    kind
                },
                parent_raw,
                parent_line,
                parent_count,
                summary_raw,
                aliases_first,
            },
        );
    }

    // Ownership through the shared resolver.
    let source = IndexedHomeSource::from_buffers(&index, &buffers);
    let mut resolver = HomeResolver::new(source);

    // Parent edge resolution for notebooks.
    let mut edges: BTreeMap<PathBuf, EdgeInner> = BTreeMap::new();
    for (relative, lite) in &lites {
        if lite.kind != LiteKind::Notebook {
            continue;
        }
        let edge = resolve_parent_edge(relative, lite, &index);
        edges.insert(relative.clone(), edge);
    }

    // Parent-kind lookup for resolved parents.
    let kind_of = |path: &Path| -> Option<LiteKind> {
        lites.get(path).map(|lite| lite.kind)
    };

    // Validate chains with memoization.
    let mut chain_cache: HashMap<PathBuf, ChainVerdict> = HashMap::new();
    let mut notebook_entries: Vec<NotebookEntry> = Vec::new();

    // Map notebook stem -> relative for child collection (case-insensitive? stems are lowercase routes).
    // Children are direct child notebooks: notebook whose resolved parent == this notebook.
    let mut children_map: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    // First pass: determine resolved parent paths for valid edges.
    let mut resolved_parents: BTreeMap<PathBuf, Option<PathBuf>> =
        BTreeMap::new();
    for (relative, edge) in &edges {
        if edge.multiple
            || edge.missing
            || edge.ambiguous
            || edge.target_path.is_none()
        {
            resolved_parents.insert(relative.clone(), None);
        } else {
            resolved_parents.insert(relative.clone(), edge.target_path.clone());
        }
    }
    for (child, parent_opt) in &resolved_parents {
        if let Some(parent) = parent_opt
            && let Some(parent_lite) = lites.get(parent)
            && parent_lite.kind == LiteKind::Notebook
        {
            let child_stem =
                lites.get(child).map(|l| l.stem.clone()).unwrap_or_default();
            children_map
                .entry(parent.clone())
                .or_default()
                .push(child_stem);
        }
    }
    for children in children_map.values_mut() {
        children.sort_by(|a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        });
    }

    // Titles needed for breadcrumbs: compute all titles first (without owner/root).
    let mut titles: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (relative, lite) in &lites {
        if lite.kind != LiteKind::Notebook {
            continue;
        }
        let body = bodies.get(relative).cloned().unwrap_or_default();
        let title =
            derive_title(&body, lite.aliases_first.as_deref(), &lite.stem);
        titles.insert(relative.clone(), title);
    }

    // Child-kind offenders: for each notebook, find area/project/ref/record children naming it.
    let mut child_kind_map: BTreeMap<PathBuf, Vec<(String, Option<usize>)>> =
        BTreeMap::new();
    for lite in lites.values() {
        if matches!(
            lite.kind,
            LiteKind::Area
                | LiteKind::Project { .. }
                | LiteKind::Ref
                | LiteKind::RecordInstance
        ) {
            let Some(parent_raw) = lite.parent_raw.as_deref() else {
                continue;
            };
            let Some(target) = wikilink_path_target(parent_raw) else {
                continue;
            };
            // Resolve with index from this child's path.
            let resolution =
                index.resolve_detailed(Some(&lite.relative), &target);
            if let Some(path) = resolution.path()
                && let Some(parent_lite) = lites.get(path)
                && parent_lite.kind == LiteKind::Notebook
            {
                child_kind_map
                    .entry(path.to_path_buf())
                    .or_default()
                    .push((lite.relative_slash.clone(), lite.parent_line));
            }
        }
    }
    for offenders in child_kind_map.values_mut() {
        offenders.sort();
    }

    for (relative, lite) in &lites {
        if lite.kind != LiteKind::Notebook {
            continue;
        }
        let body = bodies.get(relative).cloned().unwrap_or_default();
        let edge = edges.get(relative).cloned().unwrap_or(EdgeInner {
            target_path: None,
            ambiguous: false,
            missing: true,
            multiple: false,
            unquoted: false,
        });
        let mut issues: Vec<NotebookIssue> = Vec::new();

        // Summary findings.
        issues.extend(rules::summary_findings(lite.summary_raw.as_deref()));

        // Not at root.
        if relative.components().count() > 1 {
            issues.push(NotebookIssue::error(
                "notebook_not_at_root",
                None,
                "notebooks live at the vault root; capture routes are root stems",
            ));
        }

        // Parent unquoted (soft, only when parent otherwise resolves).
        if edge.unquoted && !edge.missing && !edge.ambiguous && !edge.multiple {
            issues.push(NotebookIssue::warning(
                "notebook_parent_unquoted",
                lite.parent_line,
                "quote the parent as `\"[[stem]]\"`; bare `[[stem]]` parses as a nested list",
            ));
        }

        // Parent missing / ambiguous.
        if edge.multiple || edge.ambiguous {
            let message = if edge.multiple {
                "notebook names multiple parents; keep exactly one `parent:\"[[stem]]\"`"
            } else {
                "parent is ambiguous; use a path-qualified link"
            };
            issues.push(NotebookIssue::error(
                "notebook_parent_ambiguous",
                lite.parent_line,
                message,
            ));
        } else if edge.missing || edge.target_path.is_none() {
            issues.push(NotebookIssue::error(
                "notebook_parent_missing",
                lite.parent_line,
                "notebook needs exactly one parent that resolves to an area, project, or notebook",
            ));
        } else {
            // Single resolved parent: check kind.
            let parent_path = edge.target_path.clone().expect("resolved");
            match kind_of(&parent_path) {
                Some(LiteKind::Area)
                | Some(LiteKind::Project { .. })
                | Some(LiteKind::Notebook) => {}
                Some(_) | None => {
                    issues.push(NotebookIssue::error(
                        "notebook_parent_kind",
                        lite.parent_line,
                        "parent is not an area, project, or notebook",
                    ));
                }
            }
            // Chain validation (cycles + reachability).
            let verdict =
                validate_chain(relative, &lites, &index, &mut chain_cache);
            match verdict {
                ChainVerdict::Cycle => {
                    issues.push(NotebookIssue::error(
                        "notebook_cycle",
                        lite.parent_line,
                        "parent chain cycles; break the loop",
                    ));
                }
                ChainVerdict::NoOwner => {
                    // Only add if not already reported as parent_kind on direct parent.
                    let already = issues
                        .iter()
                        .any(|issue| issue.code == "notebook_parent_kind");
                    if !already {
                        issues.push(NotebookIssue::error(
                            "notebook_parent_kind",
                            lite.parent_line,
                            "parent chain reaches no area or project",
                        ));
                    }
                }
                ChainVerdict::Ok => {}
            }
        }

        // Body rules.
        let body_start = rules::body_start_line(&body);
        let lines = rules::physical_lines(&body);
        // body_lines: (line_number, line) for lines at/after body_start.
        let body_lines: Vec<(usize, &str)> = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                let number = index + 1;
                if number >= body_start {
                    Some((number, *line))
                } else {
                    None
                }
            })
            .collect();
        issues.extend(rules::checkbox_findings(&body_lines));
        issues.extend(rules::duplicate_block_id_findings(&body_lines));

        // Title / sections / counts / last_note need body structure.
        let title = titles
            .get(relative)
            .cloned()
            .unwrap_or_else(|| lite.stem.clone());
        let (notes_total, sections, first_bullet, first_h2, h1_lines) =
            analyze_body(&body_lines);
        issues.extend(rules::shape_findings(
            &body_lines,
            first_bullet,
            first_h2,
            &h1_lines,
        ));

        // Child-kind: each offending child is an error on the notebook.
        if let Some(offenders) = child_kind_map.get(relative) {
            for (child_path, child_line) in offenders {
                issues.push(NotebookIssue::error(
                    "notebook_child_kind",
                    *child_line,
                    format!("{child_path} names this notebook as its parent; areas, projects, refs, and record notes never do"),
                ));
            }
        }

        issues = rules::dedupe(issues);

        // Ownership: invalid notebooks keep null owner/root.
        let has_error = issues
            .iter()
            .any(|issue| issue.severity == super::model::Severity::Error);
        let (owner, root) = if has_error {
            (None, None)
        } else {
            let home = resolver.resolve_note(relative);
            let owner = stem_of_path_str(&home.key);
            let root = stem_of_path_str(&home.root_parent);
            (Some(owner), Some(root))
        };

        // Breadcrumb: root-to-notebook display. Valid only; invalid keeps [title].
        let breadcrumb = if has_error {
            vec![title.clone()]
        } else {
            build_breadcrumb(
                relative,
                &lites,
                &index,
                &titles,
                &resolved_parents,
            )
        };

        // Parent / parent_kind public fields.
        let (parent, parent_kind) = match &edge.target_path {
            Some(path) => {
                let stem = stem_of(path);
                let kind = kind_of(path).map(|kind| match kind {
                    LiteKind::Area => "area",
                    LiteKind::Project { .. } => "project",
                    LiteKind::Notebook => "notebook",
                    _ => "other",
                });
                // Only expose area/project/notebook kinds; otherwise null kind.
                let kind = match kind {
                    Some("area") | Some("project") | Some("notebook") => {
                        kind.map(str::to_string)
                    }
                    _ => None,
                };
                (Some(stem), kind)
            }
            None => (None, None),
        };

        let last_note = latest_created_date(&body_lines);
        let children = children_map.get(relative).cloned().unwrap_or_default();
        let summary = lite
            .summary_raw
            .as_deref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        notebook_entries.push(NotebookEntry {
            stem: lite.stem.clone(),
            path: lite.relative_slash.clone(),
            title,
            summary,
            parent,
            parent_kind,
            owner,
            root,
            breadcrumb,
            children,
            notes: notes_total,
            sections: sections
                .into_iter()
                .map(|(title, notes)| NotebookSection { title, notes })
                .collect(),
            last_note,
            issues,
        });
    }

    // Deterministic ordering: roots and siblings by display title case-insensitively, path tie-breaker.
    // Full scan model stays available; ordering here is for reporting.
    // We sort by (root, title, path) where invalid (no root) sort last.
    notebook_entries.sort_by(|left, right| match (&left.root, &right.root) {
        (Some(a), Some(b)) => a
            .to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| {
                left.title.to_lowercase().cmp(&right.title.to_lowercase())
            })
            .then_with(|| left.path.cmp(&right.path)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left
            .title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.path.cmp(&right.path)),
    });

    let roots = notebook_entries
        .iter()
        .filter_map(|entry| entry.root.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let errors = notebook_entries
        .iter()
        .map(|entry| {
            entry
                .issues
                .iter()
                .filter(|issue| issue.severity == super::model::Severity::Error)
                .count()
        })
        .sum();
    let warnings = notebook_entries
        .iter()
        .map(|entry| {
            entry
                .issues
                .iter()
                .filter(|issue| {
                    issue.severity == super::model::Severity::Warning
                })
                .count()
        })
        .sum();
    let counts = super::model::NotebookCounts {
        notebooks: notebook_entries.len(),
        roots,
        errors,
        warnings,
    };
    ScanOutput {
        report: NotebookReport {
            notebooks: notebook_entries,
            counts,
        },
        io_errors,
    }
}

fn collect_markdown(
    root: &Path,
    directory: &Path,
    out: &mut Vec<PathBuf>,
    io_errors: &mut Vec<String>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
        Err(error) => {
            io_errors.push(format!(
                "{}: failed to read directory: {error}",
                display_slash(&relative_or(root, directory))
            ));
            return;
        }
    };
    let Ok(mut entries) = entries else {
        io_errors.push(format!(
            "{}: failed to read directory entries",
            display_slash(&relative_or(root, directory))
        ));
        return;
    };
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                io_errors.push(format!(
                    "{}: failed to inspect path: {error}",
                    display_slash(&relative_or(root, &path))
                ));
                continue;
            }
        };
        if file_type.is_dir() {
            if is_excluded_directory(&path) {
                continue;
            }
            // `_templates/` is already excluded via ALWAYS_EXCLUDED, but
            // keep the explicit guard for the notebook contract.
            if path.file_name().and_then(|name| name.to_str())
                == Some("_templates")
            {
                continue;
            }
            collect_markdown(root, &path, out, io_errors);
            continue;
        }
        if file_type.is_file() && is_markdown_file(&path) {
            out.push(relative_or(root, &path));
        }
    }
}

fn relative_or(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn display_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_string()
}

fn stem_of_path_str(value: &str) -> String {
    let trimmed = value.trim();
    let without_suffix = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".MD"))
        .unwrap_or(trimmed);
    Path::new(without_suffix)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| without_suffix.to_string())
}

fn classify_kind(
    frontmatter: &crate::native::projects::Frontmatter<'_>,
    record_ids: &BTreeSet<String>,
) -> LiteKind {
    if frontmatter_is_area(frontmatter) {
        return LiteKind::Area;
    }
    if frontmatter_is_project(frontmatter) {
        let status = crate::native::projects::ProjectStatus::parse(
            frontmatter_value(frontmatter, "status"),
        );
        return LiteKind::Project {
            terminal: status.is_terminal(),
        };
    }
    if frontmatter_is_notebook(frontmatter) {
        return LiteKind::Notebook;
    }
    let names = crate::native::projects::frontmatter_type_names(frontmatter);
    if names.iter().any(|name| name == "ref") {
        return LiteKind::Ref;
    }
    if names.iter().any(|name| record_ids.contains(name)) {
        return LiteKind::RecordInstance;
    }
    if names.is_empty() {
        return LiteKind::Untyped;
    }
    LiteKind::Other
}

fn parent_info(
    frontmatter: &crate::native::projects::Frontmatter<'_>,
) -> (Option<String>, Option<usize>, usize) {
    let mut count = 0;
    let mut first_raw = None;
    let mut first_line = None;
    for (index, line) in frontmatter.lines.iter().enumerate() {
        // Frontmatter lines are 2-based (line 1 is `---`).
        let number = index + 2;
        let Some(rest) = line.strip_prefix("parent") else {
            continue;
        };
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        count += 1;
        if first_raw.is_none() {
            first_raw = Some(value.trim().to_string());
            first_line = Some(number);
        }
    }
    (
        first_raw.filter(|value| !value.is_empty()),
        first_line,
        count,
    )
}

fn reconstruct_header(
    frontmatter: &crate::native::projects::Frontmatter<'_>,
) -> String {
    let mut header = String::from("---\n");
    for line in &frontmatter.lines {
        header.push_str(line);
        header.push('\n');
    }
    header.push_str("---\n");
    header
}

fn first_alias(contents: &str) -> Option<String> {
    let start = contents.find("---")?;
    let rest = &contents[start + 3..];
    let end = rest.find("---")?;
    let block = &rest[..end];
    let parsed: serde_yaml::Value = serde_yaml::from_str(block).ok()?;
    let mapping = parsed.as_mapping()?;
    let key = serde_yaml::Value::String("aliases".to_string());
    let value = mapping.get(&key)?;
    match value {
        serde_yaml::Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        serde_yaml::Value::Sequence(items) => items.iter().find_map(|item| {
            item.as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        }),
        _ => None,
    }
}

/// Resolve one notebook parent edge with path-aware semantics.
fn resolve_parent_edge(
    current: &Path,
    lite: &NoteFactsLite,
    index: &NoteIndex,
) -> EdgeInner {
    if lite.parent_count > 1 {
        return EdgeInner {
            target_path: None,
            ambiguous: false,
            missing: false,
            multiple: true,
            unquoted: false,
        };
    }
    let Some(raw) = lite.parent_raw.clone() else {
        return EdgeInner {
            target_path: None,
            ambiguous: false,
            missing: true,
            multiple: false,
            unquoted: false,
        };
    };
    if raw.trim().is_empty() {
        return EdgeInner {
            target_path: None,
            ambiguous: false,
            missing: true,
            multiple: false,
            unquoted: false,
        };
    }
    // Multi-parent value: flow list with more than one entry or more than
    // one wikilink in the scalar.
    if is_multi_parent_value(&raw) {
        return EdgeInner {
            target_path: None,
            ambiguous: false,
            missing: false,
            multiple: true,
            unquoted: false,
        };
    }
    let unquoted_for_missing = is_unquoted_wikilink(&raw);
    let Some(target) = wikilink_path_target(&raw) else {
        return EdgeInner {
            target_path: None,
            ambiguous: false,
            missing: true,
            multiple: false,
            unquoted: unquoted_for_missing,
        };
    };
    // Quoting: raw trimmed starts with a quote when YAML-quoted.
    let unquoted = is_unquoted_wikilink(&raw);
    match index.resolve_detailed(Some(current), &target) {
        crate::native::vault_links::LinkResolution::Found(path) => EdgeInner {
            target_path: Some(path),
            ambiguous: false,
            missing: false,
            multiple: false,
            unquoted,
        },
        crate::native::vault_links::LinkResolution::Ambiguous => EdgeInner {
            target_path: None,
            ambiguous: true,
            missing: false,
            multiple: false,
            unquoted,
        },
        crate::native::vault_links::LinkResolution::Missing => EdgeInner {
            target_path: None,
            ambiguous: false,
            missing: true,
            multiple: false,
            unquoted,
        },
    }
}

#[derive(Debug, Clone)]
struct EdgeInner {
    target_path: Option<PathBuf>,
    ambiguous: bool,
    missing: bool,
    multiple: bool,
    unquoted: bool,
}

fn is_multi_parent_value(raw: &str) -> bool {
    let trimmed = raw.trim();
    // Flow list with a comma outside quotes holds multiple parents.
    if trimmed.starts_with('[') {
        let inner =
            trimmed.trim_start_matches('[').trim_end_matches(']').trim();
        if inner.is_empty() {
            return false;
        }
        // Count top-level commas.
        let mut depth = 0;
        let mut quote = None;
        let mut commas = 0;
        for ch in inner.chars() {
            match quote {
                Some(q) if ch == q => quote = None,
                Some(_) => {}
                None => {
                    if ch == '"' || ch == '\'' {
                        quote = Some(ch);
                    } else if ch == '[' {
                        depth += 1;
                    } else if ch == ']' && depth > 0 {
                        depth -= 1;
                    } else if ch == ',' && depth == 0 {
                        commas += 1;
                    }
                }
            }
        }
        return commas > 0;
    }
    // More than one `[[...]]` in the scalar.
    trimmed.match_indices("[[").count() > 1
}

fn is_unquoted_wikilink(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return false;
    }
    trimmed.starts_with("[[") && trimmed.ends_with("]]")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainVerdict {
    Ok,
    Cycle,
    NoOwner,
}

fn validate_chain(
    start: &Path,
    lites: &BTreeMap<PathBuf, NoteFactsLite>,
    index: &NoteIndex,
    cache: &mut HashMap<PathBuf, ChainVerdict>,
) -> ChainVerdict {
    fn walk(
        current: &Path,
        lites: &BTreeMap<PathBuf, NoteFactsLite>,
        index: &NoteIndex,
        cache: &mut HashMap<PathBuf, ChainVerdict>,
        visiting: &mut Vec<PathBuf>,
    ) -> ChainVerdict {
        if let Some(cached) = cache.get(current) {
            return *cached;
        }
        if visiting.contains(&current.to_path_buf()) {
            return ChainVerdict::Cycle;
        }
        let Some(lite) = lites.get(current) else {
            return ChainVerdict::NoOwner;
        };
        match lite.kind {
            LiteKind::Area | LiteKind::Project { .. } => {
                cache.insert(current.to_path_buf(), ChainVerdict::Ok);
                return ChainVerdict::Ok;
            }
            LiteKind::Notebook => {}
            _ => {
                cache.insert(current.to_path_buf(), ChainVerdict::NoOwner);
                return ChainVerdict::NoOwner;
            }
        }
        // Notebook: follow its parent.
        if lite.parent_count > 1 {
            cache.insert(current.to_path_buf(), ChainVerdict::NoOwner);
            return ChainVerdict::NoOwner;
        }
        let Some(raw) = lite.parent_raw.as_deref() else {
            cache.insert(current.to_path_buf(), ChainVerdict::NoOwner);
            return ChainVerdict::NoOwner;
        };
        if is_multi_parent_value(raw) {
            cache.insert(current.to_path_buf(), ChainVerdict::NoOwner);
            return ChainVerdict::NoOwner;
        }
        let Some(target) = wikilink_path_target(raw) else {
            cache.insert(current.to_path_buf(), ChainVerdict::NoOwner);
            return ChainVerdict::NoOwner;
        };
        let resolution = index.resolve_detailed(Some(current), &target);
        let Some(next) = resolution.path() else {
            cache.insert(current.to_path_buf(), ChainVerdict::NoOwner);
            return ChainVerdict::NoOwner;
        };
        // Direct parent kind gates the walk.
        match lites.get(next).map(|lite| lite.kind) {
            Some(LiteKind::Area)
            | Some(LiteKind::Project { .. })
            | Some(LiteKind::Notebook) => {}
            _ => {
                cache.insert(current.to_path_buf(), ChainVerdict::NoOwner);
                return ChainVerdict::NoOwner;
            }
        }
        visiting.push(current.to_path_buf());
        let verdict = walk(next, lites, index, cache, visiting);
        visiting.pop();
        cache.insert(current.to_path_buf(), verdict);
        verdict
    }
    walk(start, lites, index, cache, &mut Vec::new())
}

fn derive_title(body: &str, aliases_first: Option<&str>, stem: &str) -> String {
    let body_start = rules::body_start_line(body);
    let lines: Vec<&str> = rules::physical_lines(body);
    let mut fence = None;
    for (index, line) in lines.iter().enumerate() {
        let number = index + 1;
        if number < body_start {
            continue;
        }
        if rules::track_fence(line, &mut fence) {
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") || trimmed == "##" {
            // An H1 after a bullet or H2 is not a title candidate.
            break;
        }
        if is_column_bullet(line) {
            break;
        }
        if let Some(title) = parse_h1(line) {
            let cleaned = clean_h1(&title);
            if !cleaned.is_empty() {
                return cleaned;
            }
        }
    }
    if let Some(alias) = aliases_first
        && !alias.trim().is_empty()
    {
        return alias.trim().to_string();
    }
    humanize_stem(stem)
}

fn parse_h1(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    // Exactly one `#` followed by space; `##` is a section.
    if !trimmed.starts_with("# ") && trimmed != "#" {
        // Only accept `# ` form to avoid `#task` false positives.
        return None;
    }
    let title = trimmed.strip_prefix('#').unwrap_or("").trim().to_string();
    (!title.is_empty()).then_some(title)
}

fn clean_h1(raw: &str) -> String {
    // Reduce wikilinks to display text.
    let mut out = String::new();
    let mut rest = raw;
    while let Some(open) = rest.find("[[") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        if let Some(close) = after.find("]]") {
            let inner = &after[..close];
            let display = inner
                .split('|')
                .nth(1)
                .or_else(|| inner.split('|').next())
                .unwrap_or(inner);
            // Strip heading suffix.
            let display = display.split('#').next().unwrap_or(display);
            // For path-qualified, keep final segment's display.
            let display = display.rsplit('/').next().unwrap_or(display);
            out.push_str(display.trim());
            rest = &after[close + 2..];
        } else {
            out.push_str(&rest[open..]);
            break;
        }
    }
    if !rest.contains("[[") {
        out.push_str(rest);
    }
    // Strip emphasis markers `*`, `_`, `` ` ``, `~~`.
    let mut cleaned = String::new();
    for ch in out.chars() {
        if matches!(ch, '*' | '_' | '`' | '~') {
            continue;
        }
        cleaned.push(ch);
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_column_bullet(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return false;
    }
    let Some(first) = line.chars().next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '+') {
        return false;
    }
    line[first.len_utf8()..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_whitespace())
}

/// Analyze body structure: total notes, sections with counts, first bullet/H2, H1 lines.
fn analyze_body(
    body_lines: &[(usize, &str)],
) -> (
    usize,
    Vec<(String, usize)>,
    Option<usize>,
    Option<usize>,
    Vec<usize>,
) {
    let mut h1_lines = Vec::new();
    let mut first_bullet = None;
    let mut first_h2 = None;
    let mut sections: Vec<(usize, String)> = Vec::new();
    let mut fence = None;
    for (number, line) in body_lines {
        if rules::track_fence(line, &mut fence) {
            continue;
        }
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("# ") || trimmed_start == "#" {
            h1_lines.push(*number);
            continue;
        }
        if trimmed_start.starts_with("## ") || trimmed_start == "##" {
            if first_h2.is_none() {
                first_h2 = Some(*number);
            }
            let title = trimmed_start
                .strip_prefix("##")
                .unwrap_or("")
                .trim()
                .to_string();
            sections.push((*number, title));
            continue;
        }
        if is_column_bullet(line) && first_bullet.is_none() {
            first_bullet = Some(*number);
        }
    }
    // Count notes per region: pre-H2 region + each H2 region.
    // Only column-zero bullets count; nested detail never increments.
    let mut counts: Vec<usize> = vec![0; sections.len() + 1];
    let mut fence = None;
    // Map line number -> region index.
    let region_for_line = |number: usize| -> usize {
        let mut region = 0;
        for (index, (h2_line, _)) in sections.iter().enumerate() {
            if number > *h2_line {
                region = index + 1;
            }
        }
        region
    };
    for (number, line) in body_lines {
        if rules::track_fence(line, &mut fence) {
            continue;
        }
        if is_column_bullet(line) {
            // Exclude checkbox lines from note counts? They are errors;
            // still count them as notes so counts stay stable? The contract
            // says counts are note bullets; checkboxes are not notes. Do not
            // count checkbox rows toward notes.
            if rules::is_checkbox_line(line) {
                continue;
            }
            let region = region_for_line(*number);
            counts[region] += 1;
        }
    }
    let total: usize = counts.iter().sum();
    let section_counts: Vec<(String, usize)> = sections
        .into_iter()
        .enumerate()
        .map(|(index, (_, title))| (title, counts[index + 1]))
        .collect();
    (total, section_counts, first_bullet, first_h2, h1_lines)
}

fn latest_created_date(body_lines: &[(usize, &str)]) -> Option<String> {
    let mut latest: Option<NaiveDate> = None;
    let mut latest_raw: Option<String> = None;
    let mut fence = None;
    for (_, line) in body_lines {
        if rules::track_fence(line, &mut fence) {
            continue;
        }
        for date in extract_created_dates(line) {
            if latest.is_none_or(|current| date > current) {
                latest = Some(date);
                latest_raw = Some(date.format("%Y-%m-%d").to_string());
            }
        }
    }
    latest_raw
}

fn extract_created_dates(line: &str) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    // `[created::2026-11-09]` with optional time suffix; take date prefix.
    let mut search = line;
    while let Some(start) = search.find("[created::") {
        let after = &search[start + "[created::".len()..];
        if let Some(end) = after.find(']') {
            let raw = after[..end].trim();
            // Accept `YYYY-MM-DD` prefix; ignore time suffix.
            if raw.len() >= 10 {
                let date_part = &raw[..10];
                if let Ok(date) =
                    NaiveDate::parse_from_str(date_part, "%Y-%m-%d")
                {
                    // Validate the 10-char shape is a real calendar date.
                    dates.push(date);
                }
            }
            search = &after[end + 1..];
        } else {
            break;
        }
    }
    dates
}

fn build_breadcrumb(
    notebook: &Path,
    lites: &BTreeMap<PathBuf, NoteFactsLite>,
    index: &NoteIndex,
    titles: &BTreeMap<PathBuf, String>,
    resolved_parents: &BTreeMap<PathBuf, Option<PathBuf>>,
) -> Vec<String> {
    // Walk parents up to the root, then reverse.
    let mut chain: Vec<PathBuf> = vec![notebook.to_path_buf()];
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    visited.insert(notebook.to_path_buf());
    let mut current = notebook.to_path_buf();
    for _ in 0..16 {
        let Some(parent) =
            resolved_parents.get(&current).and_then(|opt| opt.clone())
        else {
            break;
        };
        if !visited.insert(parent.clone()) {
            break;
        }
        chain.push(parent.clone());
        // Stop at area/project (owner). Root is derived from resolver, but
        // for breadcrumb we stop at the owner and prepend root handling below.
        if let Some(lite) = lites.get(&parent)
            && matches!(lite.kind, LiteKind::Area | LiteKind::Project { .. })
        {
            break;
        }
        current = parent;
    }
    chain.reverse();
    // Convert to display: for area/project use stem? For notebooks use title.
    // The first crumb when the owner is an area/project: use its stem as in
    // the contract example (`dev`), otherwise titles.
    let mut crumbs = Vec::new();
    for path in &chain {
        if let Some(lite) = lites.get(path) {
            match lite.kind {
                LiteKind::Area | LiteKind::Project { .. } => {
                    crumbs.push(lite.stem.clone());
                }
                LiteKind::Notebook => {
                    crumbs.push(
                        titles
                            .get(path)
                            .cloned()
                            .unwrap_or_else(|| lite.stem.clone()),
                    );
                }
                _ => {
                    crumbs.push(lite.stem.clone());
                }
            }
        }
    }
    // Ensure the notebook itself is last with its title.
    if crumbs.is_empty()
        && let Some(title) = titles.get(notebook)
    {
        crumbs.push(title.clone());
    }
    let _ = (index, bob_env::bob_dir());
    crumbs
}

#[allow(dead_code)]
type ScanEdge = EdgeInner;
