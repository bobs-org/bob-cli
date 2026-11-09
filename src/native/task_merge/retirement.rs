//! Snapshot-only dependency retirement for task merges.
//!
//! The payload phase copies authored children into the destination container
//! and leaves every source tombstone in place. This module repoints open
//! dependents from each source to the target task, validates the proposed
//! batch graph for cycles, stages relation (or Cancel Log) provenance,
//! composes `plan_task_cancel` per source, and reports prerequisites and
//! orphaned successors. It also owns the idempotency classifier the
//! integration phase consults before copying.
//!
//! All helpers are pure: they read the supplied snapshot plus the staged
//! note map and never touch the filesystem, clock, or configuration.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::{
    super::{
        capture_pomodoros,
        note_tasks::{self, NoteTaskSettings},
        pomodoro,
        relation_labels::relation_label_key,
        task_cancel,
        task_dependencies::{
            self, block_link_spans, blocks as blocks_mod,
            format as dependency_format, parse as dependency_parse,
        },
        task_locator::TaskLocator,
        task_move::VaultSnapshot,
        task_relations::{
            self as task_relations, format as relation_format,
            label::normalize_relation_label, parse as relation_parse,
            plan as relation_plan,
        },
        task_status_hooks::{self, scan_pomodoros},
        vault_links::{LinkResolution, NoteIndex},
    },
    types::{
        MergeContainer, MergeContext, MergeError, MergeGesture, MergeRequest,
        PreparedMerge, SourceDisposition,
    },
};

/// One open task that waits on a merge source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct OpenDependent {
    pub(crate) path: PathBuf,
    pub(crate) block_id: Option<String>,
    pub(crate) description: String,
    pub(crate) line_index: usize,
}

/// How a source resolves against already-retired state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceOutcome {
    /// Fresh work: repoint, provenance, and cancel.
    New,
    /// Cancelled with a matching merge edge (or section log entry).
    Unchanged,
    /// Closed without a matching edge; Label gestures repair the
    /// relation only.
    AlreadyClosed,
}

/// Per-source retirement reports using contract copy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RetirementReports {
    pub(crate) prerequisites_left: Vec<String>,
    pub(crate) successors_orphaned: Vec<String>,
    pub(crate) dependents_repointed: Vec<String>,
    pub(crate) links_pruned: usize,
    pub(crate) pruned_entries: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

/// Normalize the caller-supplied merge label with the shared relation
/// rules. An empty label falls back to `merged into`.
pub(crate) fn normalize_merge_label(raw: &str) -> Result<String, MergeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok("merged into".to_string());
    }
    normalize_relation_label(trimmed).map_err(|error| {
        MergeError::new(
            "merge_invalid_label",
            format!("merge label is {}: {trimmed}", error.as_str()),
        )
    })
}

/// Canonical section provenance reason for a note plus heading.
pub(crate) fn section_provenance_reason(note: &str, heading: &str) -> String {
    format!("📥 merged into [[{note}#{heading}]]")
}

/// Resolve a Depends-On link `(target, block)` to a vault identity.
/// Empty targets resolve to the dependent's own note; missing or
/// ambiguous notes return `None` so callers preserve them
/// conservatively.
fn resolve_dependency_identity(
    index: &NoteIndex,
    dependent_path: &Path,
    target: &str,
    block_id: &str,
) -> Option<(PathBuf, String)> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Some((dependent_path.to_path_buf(), block_id.to_string()));
    }
    match index.resolve_detailed(Some(dependent_path), trimmed) {
        LinkResolution::Found(path) => Some((path, block_id.to_string())),
        LinkResolution::Ambiguous | LinkResolution::Missing => None,
    }
}

/// Scan every note for open tasks whose accepted Depends-On line names
/// one of `sources`. Malformed lines, code spans, and unresolvable
/// targets are skipped conservatively.
pub(crate) fn find_open_dependents(
    notes: &BTreeMap<PathBuf, String>,
    sources: &BTreeSet<(PathBuf, String)>,
    context: &MergeContext<'_>,
) -> Vec<OpenDependent> {
    let index = context.note_index.with_additional(notes.keys().cloned());
    let mut dependents = Vec::new();
    for (path, contents) in notes {
        let lines: Vec<&str> = contents.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
        let scan = note_tasks::scan(contents, context.task_settings);
        for task in scan.tasks() {
            if !task.status_type.is_open() {
                continue;
            }
            let Some(child) = dependency_parse::dependency_child_of(
                &lines,
                &fenced,
                task.line_index,
            ) else {
                continue;
            };
            let dependency_parse::DependencyLine::Accepted { links, .. } =
                &child.parsed
            else {
                continue;
            };
            let mut waits_on_source = false;
            for link in links {
                let Some(identity) = resolve_dependency_identity(
                    &index,
                    path,
                    &link.target,
                    &link.block_id,
                ) else {
                    continue;
                };
                if sources.contains(&identity) {
                    waits_on_source = true;
                    break;
                }
            }
            if waits_on_source {
                dependents.push(OpenDependent {
                    path: path.clone(),
                    block_id: task.block_id.clone(),
                    description: task.description.clone(),
                    line_index: task.line_index,
                });
            }
        }
    }
    dependents.sort();
    dependents.dedup();
    dependents
}

/// Whether `from` reaches `to` following `edges`.
fn reaches(
    edges: &BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
    from: &(PathBuf, String),
    to: &(PathBuf, String),
) -> bool {
    let mut visited: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    let mut stack = vec![from.clone()];
    while let Some(node) = stack.pop() {
        if node == *to && node != *from {
            return true;
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        if let Some(next) = edges.get(&node) {
            for target in next {
                if target == to {
                    return true;
                }
                if !visited.contains(target) {
                    stack.push(target.clone());
                }
            }
        }
    }
    false
}

/// Validate the proposed batch graph: every `D -> T` repoint must not
/// let `T` reach `D`. Existing unrelated cycles never refuse an
/// unchanged repeat because only proposed edges are tested.
pub(crate) fn validate_repoint_cycles(
    notes: &BTreeMap<PathBuf, String>,
    repoints: &[((PathBuf, String), (PathBuf, String))],
    sources: &BTreeSet<(PathBuf, String)>,
    context: &MergeContext<'_>,
) -> Result<(), MergeError> {
    let mut edges =
        task_status_hooks::merge_dependency_edges(notes, context.task_settings);
    for (dependent, target) in repoints {
        let targets = edges.entry(dependent.clone()).or_default();
        targets.retain(|current| !sources.contains(current));
        targets.insert(target.clone());
    }
    for (dependent, target) in repoints {
        if reaches(&edges, target, dependent) {
            return Err(MergeError::new(
                "merge_dependency_cycle",
                format!(
                    "“{}” would wait on “{}”, which already waits on it",
                    display_identity(dependent),
                    display_identity(target),
                ),
            ));
        }
    }
    Ok(())
}

fn display_identity(identity: &(PathBuf, String)) -> String {
    format!("{}#^{}", identity.0.display(), identity.1)
}

/// Repoint every open dependent of `sources` (except `target`) to
/// `target` inside `staged`. Depends-On links are deduplicated by
/// resolved identity while preserving order; canonical child lines,
/// `[dependsOn::]`/`[id::]` fields, and both BLOCKS mirrors are
/// updated. Malformed dependency content is preserved untouched.
#[allow(clippy::too_many_arguments)]
pub(crate) fn repoint_dependents(
    staged: &mut BTreeMap<PathBuf, String>,
    dependents: &[OpenDependent],
    sources: &BTreeSet<(PathBuf, String)>,
    target: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Result<(Vec<String>, Vec<String>), MergeError> {
    let mut repointed = Vec::new();
    let mut warnings = Vec::new();
    // Snapshot the index once; staged paths never change, only bytes.
    let index = context.note_index.with_additional(staged.keys().cloned());
    for dependent in dependents {
        let Some(block_id) = dependent.block_id.as_deref() else {
            return Err(MergeError::new(
                "merge_dependent_missing_id",
                format!(
                    "“{}” in {}:{} depends on a merge source but has no block id; add an id before merging",
                    dependent.description,
                    dependent.path.display(),
                    dependent.line_index + 1,
                ),
            ));
        };
        let identity = (dependent.path.clone(), block_id.to_string());
        if identity == *target {
            continue;
        }
        let Some(contents) = staged.get(&dependent.path).cloned() else {
            continue;
        };
        let lines: Vec<&str> = contents.lines().collect();
        let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
        let scan = note_tasks::scan(&contents, context.task_settings);
        let Some(task) = scan
            .tasks()
            .iter()
            .find(|task| task.block_id.as_deref() == Some(block_id))
        else {
            continue;
        };
        if !task.status_type.is_open() {
            continue;
        }
        let Some(child) = dependency_parse::dependency_child_of(
            &lines,
            &fenced,
            task.line_index,
        ) else {
            continue;
        };
        let dependency_parse::DependencyLine::Accepted { links, .. } =
            &child.parsed
        else {
            // Preserve malformed/uncertain content conservatively.
            continue;
        };
        let original_child =
            lines.get(child.line_index).unwrap_or(&"").to_string();
        let spans = block_link_spans(&original_child);
        if spans.len() != links.len() {
            return Err(MergeError::new(
                "merge_dependency_uncertain",
                format!(
                    "cannot safely repoint dependencies for ^{} in {}: parsed link spans do not match",
                    block_id,
                    dependent.path.display(),
                ),
            ));
        }
        let mut ordered: Vec<(PathBuf, String)> = Vec::new();
        let mut seen: BTreeSet<(PathBuf, String)> = BTreeSet::new();
        let mut link_texts = Vec::new();
        let mut has_unresolved = false;
        for (link, span) in links.iter().zip(&spans) {
            let Some(resolved) = resolve_dependency_identity(
                &index,
                &dependent.path,
                &link.target,
                &link.block_id,
            ) else {
                has_unresolved = true;
                link_texts.push(
                    original_child[span.full_start..span.full_end].to_string(),
                );
                warnings.push(format!(
                    "kept unresolved or ambiguous dependency [[{}#^{}]] on {}#^{}",
                    link.target,
                    link.block_id,
                    dependent.path.display(),
                    block_id,
                ));
                continue;
            };
            let mapped = if sources.contains(&resolved) {
                target.clone()
            } else {
                resolved
            };
            if seen.insert(mapped.clone()) {
                link_texts.push(dependency_format::canonical_link(
                    &mapped.0,
                    &mapped.1,
                    &dependent.path,
                    &index,
                ));
                ordered.push(mapped);
            }
        }
        // Drop the dependent's own identity if a self-edge would form;
        // the cycle validator refuses the batch first, so reaching here
        // means the self reference came from deduplication order.
        let mut note_lines: Vec<String> =
            contents.lines().map(str::to_string).collect();
        if link_texts.is_empty() {
            note_lines.remove(child.line_index);
        } else {
            note_lines[child.line_index] =
                task_status_hooks::rebuild_child_line(
                    &original_child,
                    &link_texts,
                );
        }
        // Rebuild derived fields from the kept dependency ids.
        let task_line =
            note_lines.get(task.line_index).cloned().unwrap_or_default();
        let metadata =
            task_status_hooks::task_metadata(&task_line, Some(block_id));
        let mut kept_field_ids: Vec<String> = Vec::new();
        let mut seen_fields = BTreeSet::new();
        for (path, block) in &ordered {
            let Ok(id) = task_dependencies::dependency_id(path, block) else {
                continue;
            };
            if seen_fields.insert(id.clone()) {
                kept_field_ids.push(id);
            }
        }
        // R4 keeps unaccounted [dependsOn::] values as healing breadcrumbs
        // while any accepted line target remains unresolved. Mirror that
        // production policy instead of erasing metadata whose identity is
        // still uncertain.
        if has_unresolved {
            for id in &metadata.depends_on {
                if seen_fields.insert(id.clone()) {
                    kept_field_ids.push(id.clone());
                }
            }
        }
        // Preserve the existing task id; only the dependsOn list
        // changes. The child-line edit never touches the task line's
        // own fields, so the pre-edit metadata still holds the id.
        let updated_task_line = task_status_hooks::set_task_fields(
            &task_line,
            metadata.task_id.as_deref(),
            &kept_field_ids,
        );
        note_lines[task.line_index] = updated_task_line;
        let ending = dependency_format::note_line_ending(&contents);
        let final_newline = dependency_format::has_final_newline(&contents);
        let mut updated = note_lines.join(ending);
        if final_newline && !note_lines.is_empty() {
            updated.push_str(ending);
        }
        staged.insert(dependent.path.clone(), updated);
        // BLOCKS mirrors: detach from every source, attach to target.
        for source in sources {
            remove_blocks_entry(staged, source, &identity, context);
        }
        add_blocks_entry(staged, target, &identity, context)?;
        repointed.push(format!("{}#^{}", dependent.path.display(), block_id));
    }
    repointed.sort();
    repointed.dedup();
    warnings.sort();
    warnings.dedup();
    Ok((repointed, warnings))
}

/// Read the `[id::]` of the task at `line_index` by re-scanning the
/// Remove `dependent` from `owner`'s BLOCKS line, dropping the line when
/// it becomes empty. Malformed BLOCKS lines are left alone.
fn remove_blocks_entry(
    staged: &mut BTreeMap<PathBuf, String>,
    owner: &(PathBuf, String),
    dependent: &(PathBuf, String),
    context: &MergeContext<'_>,
) {
    let Some(contents) = staged.get(&owner.0).cloned() else {
        return;
    };
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return;
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let scan = note_tasks::scan(&contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(owner.1.as_str()))
    else {
        return;
    };
    let children =
        blocks_mod::blocks_children_of(&lines, &fenced, task.line_index);
    let Some(child) = children.first() else {
        return;
    };
    let blocks_mod::BlocksLine::Accepted { links, .. } = &child.parsed else {
        return;
    };
    let index = context.note_index.with_additional(staged.keys().cloned());
    let mut kept: Vec<String> = Vec::new();
    let mut removed = false;
    for link in links {
        let resolved = resolve_dependency_identity(
            &index,
            &owner.0,
            &link.target,
            &link.block_id,
        );
        if resolved.as_ref() == Some(dependent) {
            removed = true;
            continue;
        }
        kept.push(if link.target.is_empty() {
            format!("[[#^{}]]", link.block_id)
        } else {
            format!("[[{}#^{}]]", link.target, link.block_id)
        });
    }
    if !removed {
        return;
    }
    let mut note_lines: Vec<String> =
        contents.lines().map(str::to_string).collect();
    if kept.is_empty() {
        note_lines.remove(child.line_index);
    } else {
        let original = lines.get(child.line_index).unwrap_or(&"").to_string();
        let indent_len = original
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let indent = original[..indent_len].to_string();
        note_lines[child.line_index] =
            blocks_mod::format_blocks_line(&indent, &kept);
    }
    let ending = dependency_format::note_line_ending(&contents);
    let final_newline = dependency_format::has_final_newline(&contents);
    let mut updated = note_lines.join(ending);
    if final_newline && !note_lines.is_empty() {
        updated.push_str(ending);
    }
    staged.insert(owner.0.clone(), updated);
}

/// Remove generated prerequisite mirrors for a dependent once it is
/// cancelled. Its Depends-On child remains on the tombstone as history;
/// reconciliation no longer projects a closed dependent into BLOCKS.
fn remove_cancelled_prerequisite_mirrors(
    staged: &mut BTreeMap<PathBuf, String>,
    dependent: &(PathBuf, String),
    context: &MergeContext<'_>,
) {
    let Some(contents) = staged.get(&dependent.0).cloned() else {
        return;
    };
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return;
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let scan = note_tasks::scan(&contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(dependent.1.as_str()))
    else {
        return;
    };
    let Some(child) =
        dependency_parse::dependency_child_of(&lines, &fenced, task.line_index)
    else {
        return;
    };
    let dependency_parse::DependencyLine::Accepted { links, .. } =
        &child.parsed
    else {
        return;
    };
    let index = context.note_index.with_additional(staged.keys().cloned());
    let prerequisites = links
        .iter()
        .filter_map(|link| {
            resolve_dependency_identity(
                &index,
                &dependent.0,
                &link.target,
                &link.block_id,
            )
        })
        .collect::<BTreeSet<_>>();
    for prerequisite in prerequisites {
        remove_blocks_entry(staged, &prerequisite, dependent, context);
    }
}

/// Add `dependent` to `owner`'s BLOCKS line, creating the line when
/// absent. Entries are deduplicated by resolved identity.
fn add_blocks_entry(
    staged: &mut BTreeMap<PathBuf, String>,
    owner: &(PathBuf, String),
    dependent: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Result<(), MergeError> {
    let Some(contents) = staged.get(&owner.0).cloned() else {
        return Ok(());
    };
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return Ok(());
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let scan = note_tasks::scan(&contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(owner.1.as_str()))
    else {
        return Ok(());
    };
    let index = context.note_index.with_additional(staged.keys().cloned());
    let children =
        blocks_mod::blocks_children_of(&lines, &fenced, task.line_index);
    if let Some(child) = children.first() {
        let blocks_mod::BlocksLine::Accepted { links, .. } = &child.parsed
        else {
            return Ok(());
        };
        for link in links {
            let resolved = resolve_dependency_identity(
                &index,
                &owner.0,
                &link.target,
                &link.block_id,
            );
            if resolved.as_ref() == Some(dependent) {
                return Ok(());
            }
        }
        let mut kept: Vec<String> = links
            .iter()
            .map(|link| {
                if link.target.is_empty() {
                    format!("[[#^{}]]", link.block_id)
                } else {
                    format!("[[{}#^{}]]", link.target, link.block_id)
                }
            })
            .collect();
        kept.push(dependency_format::canonical_link(
            &dependent.0,
            &dependent.1,
            &owner.0,
            &index,
        ));
        blocks_mod::sort_blocks_links(&mut kept);
        kept.dedup();
        let original = lines.get(child.line_index).unwrap_or(&"").to_string();
        let indent_len = original
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let indent = original[..indent_len].to_string();
        let mut note_lines: Vec<String> =
            contents.lines().map(str::to_string).collect();
        note_lines[child.line_index] =
            blocks_mod::format_blocks_line(&indent, &kept);
        let ending = dependency_format::note_line_ending(&contents);
        let final_newline = dependency_format::has_final_newline(&contents);
        let mut updated = note_lines.join(ending);
        if final_newline && !note_lines.is_empty() {
            updated.push_str(ending);
        }
        staged.insert(owner.0.clone(), updated);
        return Ok(());
    }
    // No BLOCKS line: insert one at the canonical slot.
    let slot = blocks_mod::blocks_insert_slot(&lines, &fenced, task.line_index);
    let link = dependency_format::canonical_link(
        &dependent.0,
        &dependent.1,
        &owner.0,
        &index,
    );
    let text = blocks_mod::format_blocks_line(&slot.indent, &[link]);
    let mut note_lines: Vec<String> =
        contents.lines().map(str::to_string).collect();
    let at = slot.line_index.min(note_lines.len());
    note_lines.insert(at, text);
    let ending = dependency_format::note_line_ending(&contents);
    let final_newline = dependency_format::has_final_newline(&contents);
    let mut updated = note_lines.join(ending);
    if final_newline && !note_lines.is_empty() {
        updated.push_str(ending);
    }
    staged.insert(owner.0.clone(), updated);
    Ok(())
}

/// Collect a source's own open prerequisites (retained, not
/// transferred) as `note#^id` display strings.
pub(crate) fn source_prerequisites(
    notes: &BTreeMap<PathBuf, String>,
    source: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Vec<String> {
    let Some(contents) = notes.get(&source.0) else {
        return Vec::new();
    };
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let scan = note_tasks::scan(contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(source.1.as_str()))
    else {
        return Vec::new();
    };
    let Some(child) =
        dependency_parse::dependency_child_of(&lines, &fenced, task.line_index)
    else {
        return Vec::new();
    };
    let dependency_parse::DependencyLine::Accepted { links, .. } =
        &child.parsed
    else {
        return Vec::new();
    };
    let index = context.note_index.with_additional(notes.keys().cloned());
    let mut out = Vec::new();
    for link in links {
        let Some((path, block)) = resolve_dependency_identity(
            &index,
            &source.0,
            &link.target,
            &link.block_id,
        ) else {
            continue;
        };
        // Report open prerequisites; closed ones need no hand-add hint.
        if let Some(target_contents) = notes.get(&path) {
            let target_scan =
                note_tasks::scan(target_contents, context.task_settings);
            let open = target_scan.tasks().iter().any(|candidate| {
                candidate.block_id.as_deref() == Some(block.as_str())
                    && candidate.status_type.is_open()
            });
            if open {
                out.push(format!("{}#^{}", path.display(), block));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Collect tasks whose LINKS line carries a `starts after` entry
/// pointing at `source`. Those successors are untouched and reported.
pub(crate) fn successor_orphans(
    notes: &BTreeMap<PathBuf, String>,
    source: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> Vec<String> {
    let index = context.note_index.with_additional(notes.keys().cloned());
    let wanted_keys = context
        .successor_labels
        .iter()
        .map(|label| label.key.clone())
        .collect::<BTreeSet<_>>();
    if wanted_keys.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (path, contents) in notes {
        let lines: Vec<&str> = contents.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
        let scan = note_tasks::scan(contents, context.task_settings);
        for task in scan.tasks() {
            let children = relation_parse::links_children_of(
                &lines,
                &fenced,
                task.line_index,
            );
            for child in &children {
                let task_relations::LinksLine::Accepted { entries, .. } =
                    &child.parsed
                else {
                    continue;
                };
                for entry in entries {
                    let label_key = entry
                        .label
                        .as_deref()
                        .map(relation_label_key)
                        .unwrap_or_default();
                    if !wanted_keys.contains(&label_key) {
                        continue;
                    }
                    let resolved = if entry.target.is_empty() {
                        Some((path.clone(), entry.block_id.clone()))
                    } else {
                        match index.resolve_detailed(Some(path), &entry.target)
                        {
                            LinkResolution::Found(resolved) => {
                                Some((resolved, entry.block_id.clone()))
                            }
                            LinkResolution::Ambiguous
                            | LinkResolution::Missing => None,
                        }
                    };
                    if resolved.as_ref() == Some(source) {
                        out.push(format!(
                            "{}#^{}",
                            path.display(),
                            task.block_id.as_deref().unwrap_or("?")
                        ));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Map prune slots to distinct open Pomodoro entry names from the daily
/// preimage. Names follow the ledger order and are deduplicated; empty
/// names are dropped so callers fall back to the aggregate copy.
fn pruned_entry_names(
    daily_preimage: Option<&str>,
    slots: &[crate::native::task_cancel::prune::PrunedLinkSlot],
) -> Vec<String> {
    let Some(preimage) = daily_preimage else {
        return Vec::new();
    };
    let lines = task_status_hooks::logical_lines(preimage);
    let Some(section) = pomodoro::pomodoros_section_range(&lines) else {
        return Vec::new();
    };
    let model = scan_pomodoros(&lines, section);
    let capture = capture_pomodoros::scan(preimage);
    let mut names_by_line = std::collections::BTreeMap::new();
    for entry in &capture.entries {
        names_by_line.insert(
            entry.line.saturating_sub(1),
            entry.name.clone().unwrap_or_default(),
        );
    }
    let mut names = Vec::new();
    for slot in slots {
        let Some(entry) = model.entries.get(slot.entry_index) else {
            continue;
        };
        if !entry.open {
            continue;
        }
        let name = names_by_line
            .get(&entry.line_index)
            .cloned()
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Classify one source against the pre-payload snapshot for
/// idempotency and closed-source handling.
///
/// * `New` — open, or closed but explicit merges must refuse.
/// * `Unchanged` — cancelled with a matching merge edge (task) or
///   section log entry (section).
/// * `AlreadyClosed` — any other closed source; Label gestures repair
///   the relation only.
pub(crate) fn classify_source(
    snapshot: &VaultSnapshot,
    source: &TaskLocator,
    target: &MergeContainer,
    label: &str,
    context: &MergeContext<'_>,
) -> Result<(SourceOutcome, SourceDisposition), MergeError> {
    let located = super::containers::resolve_task(snapshot, source, context)?;
    let open = located.task.status_type.is_open();
    if open {
        return Ok((SourceOutcome::New, SourceDisposition::Open));
    }
    // Closed: look for a matching merge edge before refusing.
    let label_key = relation_label_key(label);
    let matches_task_edge = match target {
        MergeContainer::Task(target_locator)
        | MergeContainer::Drawer(target_locator, _) => source_links_match(
            snapshot,
            &located.path,
            &located.task.line_index,
            target_locator,
            &label_key,
            context,
        )?,
        MergeContainer::Section { note, selector } => section_log_matches(
            snapshot,
            &located.path,
            &located.task.line_index,
            note,
            selector,
            context,
        ),
    };
    if matches_task_edge
        && located.task.status_type == note_tasks::TaskStatusType::Cancelled
    {
        return Ok((SourceOutcome::Unchanged, SourceDisposition::Closed));
    }
    Ok((SourceOutcome::AlreadyClosed, SourceDisposition::Closed))
}

/// The `merge_source_closed` refusal for an explicit merge of a closed
/// source: the already-merged teaching copy when an existing managed merge
/// relation resolves, otherwise a truthful generic closed-source message.
/// Capture's read-only `merge_in_pending` shares it with execution.
pub(crate) fn closed_source_refusal(
    snapshot: &VaultSnapshot,
    source_path: &Path,
    source_block_id: &str,
    task_settings: &NoteTaskSettings,
    note_index: &NoteIndex,
) -> MergeError {
    let message = existing_merge_target_message(
        snapshot,
        source_path,
        source_block_id,
        task_settings,
        note_index,
    )
    .unwrap_or_else(|| format!("^{source_block_id} is closed"));
    MergeError::new("merge_source_closed", message)
}

/// Teaching copy for an already-merged closed source: name its cleaned
/// title, the existing merge target title, and the target note. Returns
/// `None` when no existing managed merge relation resolves, so callers
/// keep a truthful generic closed-source error. Uses the normal parser
/// and index with configured label normalization; never mints IDs.
fn existing_merge_target_message(
    snapshot: &VaultSnapshot,
    source_path: &Path,
    source_block_id: &str,
    task_settings: &NoteTaskSettings,
    note_index: &NoteIndex,
) -> Option<String> {
    let contents = snapshot.notes.get(source_path)?;
    let scan = note_tasks::scan(contents, task_settings);
    let source_task = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(source_block_id))?;
    let source_title = note_tasks::clean_description(
        &source_task.description,
        &task_settings.global_filter,
        source_task.block_id.as_deref(),
    );
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children = relation_parse::links_children_of(
        &lines,
        &fenced,
        source_task.line_index,
    );
    let index = note_index.with_additional(snapshot.notes.keys().cloned());
    for child in &children {
        let task_relations::LinksLine::Accepted { entries, .. } = &child.parsed
        else {
            continue;
        };
        for entry in entries {
            let resolved = if entry.target.is_empty() {
                Some((source_path.to_path_buf(), entry.block_id.clone()))
            } else {
                match index.resolve_detailed(Some(source_path), &entry.target) {
                    LinkResolution::Found(path) => {
                        Some((path, entry.block_id.clone()))
                    }
                    LinkResolution::Ambiguous | LinkResolution::Missing => None,
                }
            };
            let Some((target_path, target_block)) = resolved else {
                continue;
            };
            let Some(target_contents) = snapshot.notes.get(&target_path) else {
                continue;
            };
            let target_scan = note_tasks::scan(target_contents, task_settings);
            let Some(target_task) = target_scan.tasks().iter().find(|task| {
                task.block_id.as_deref() == Some(target_block.as_str())
            }) else {
                continue;
            };
            let target_title = note_tasks::clean_description(
                &target_task.description,
                &task_settings.global_filter,
                target_task.block_id.as_deref(),
            );
            if target_title.is_empty() {
                continue;
            }
            let display = target_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| target_path.to_string_lossy().into_owned());
            return Some(format!(
                "“{source_title}” is already merged into “{target_title}” ({display})"
            ));
        }
    }
    None
}

fn source_links_match(
    snapshot: &VaultSnapshot,
    source_path: &Path,
    source_line: &usize,
    target: &TaskLocator,
    label_key: &str,
    context: &MergeContext<'_>,
) -> Result<bool, MergeError> {
    let contents = snapshot.notes.get(source_path).ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", source_path.display()),
        )
    })?;
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children =
        relation_parse::links_children_of(&lines, &fenced, *source_line);
    let index = context
        .note_index
        .with_additional(snapshot.notes.keys().cloned());
    // Resolve the target locator to a vault identity for comparison.
    let target_located =
        super::containers::resolve_task(snapshot, target, context)?;
    let target_identity = (
        target_located.path,
        target_located.task.block_id.clone().unwrap_or_default(),
    );
    for child in &children {
        let task_relations::LinksLine::Accepted { entries, .. } = &child.parsed
        else {
            continue;
        };
        for entry in entries {
            let entry_key = entry
                .label
                .as_deref()
                .map(relation_label_key)
                .unwrap_or_default();
            if entry_key != *label_key {
                continue;
            }
            let resolved = if entry.target.is_empty() {
                Some((source_path.to_path_buf(), entry.block_id.clone()))
            } else {
                match index.resolve_detailed(Some(source_path), &entry.target) {
                    LinkResolution::Found(path) => {
                        Some((path, entry.block_id.clone()))
                    }
                    LinkResolution::Ambiguous | LinkResolution::Missing => None,
                }
            };
            if resolved.as_ref() == Some(&target_identity) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn section_log_matches(
    snapshot: &VaultSnapshot,
    source_path: &Path,
    source_line: &usize,
    note: &str,
    selector: &super::super::capture_language::DrawerSelector,
    context: &MergeContext<'_>,
) -> bool {
    let Some(contents) = snapshot.notes.get(source_path) else {
        return false;
    };
    let scan = note_tasks::scan(contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.line_index == *source_line)
    else {
        return false;
    };
    let Ok(note_path) = section_note_path(snapshot, note, context) else {
        return false;
    };
    let Some(note_contents) = snapshot.notes.get(&note_path) else {
        return false;
    };
    let Ok(heading) =
        super::containers::section_selector_title(note_contents, selector)
    else {
        return false;
    };
    let note_target =
        note_path.with_extension("").to_string_lossy().into_owned();
    let reason = section_provenance_reason(&note_target, &heading);
    cancel_log_has_reason(contents, task.line_index, task.block_end, &reason)
}

/// Match one dated reason only inside this task's direct managed Cancel Log.
/// Prose, fences, and later sibling tasks cannot establish merge identity.
fn cancel_log_has_reason(
    contents: &str,
    task_line: usize,
    task_block_end: usize,
    reason: &str,
) -> bool {
    let lines = task_status_hooks::logical_lines(contents);
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let direct = blocks_mod::direct_child_lines(&lines, &fenced, task_line);
    let Some(log_position) = direct
        .iter()
        .position(|index| blocks_mod::is_cancel_log(lines[*index]))
    else {
        return false;
    };
    let log_line = direct[log_position];
    let end_line = direct.get(log_position + 1).copied().unwrap_or_else(|| {
        crate::native::capture::line_spans(contents)
            .iter()
            .position(|line| line.end > task_block_end)
            .unwrap_or(lines.len())
    });
    (log_line + 1..end_line).any(|line_index| {
        if fenced.contains(&line_index) {
            return false;
        }
        let line = lines[line_index].trim_start();
        let Some(item) = line.strip_prefix("- ") else {
            return false;
        };
        let Some(dated) = item.strip_prefix('*') else {
            return false;
        };
        dated
            .split_once("* — ")
            .is_some_and(|(date, entry_reason)| {
                !date.is_empty() && entry_reason == reason
            })
    })
}

fn section_note_path(
    snapshot: &VaultSnapshot,
    note: &str,
    context: &MergeContext<'_>,
) -> Result<PathBuf, MergeError> {
    let index = context
        .note_index
        .with_additional(snapshot.notes.keys().cloned());
    match index.resolve_detailed(None, note) {
        LinkResolution::Found(path) => Ok(path),
        LinkResolution::Ambiguous => Err(MergeError::new(
            "ambiguous_note",
            format!("`{note}` matches more than one note"),
        )),
        LinkResolution::Missing => Err(MergeError::new(
            "task_not_found",
            format!("no note matches `{note}`"),
        )),
    }
}

/// Stage relation provenance for one source: `merged into [[T]]` on S
/// plus `[[S]] (merged into)` on T's LINKED FROM. Sections use a Cancel
/// Log entry instead; no task endpoint is invented.
pub(crate) fn stage_task_provenance(
    staged: &mut BTreeMap<PathBuf, String>,
    source: &(PathBuf, String),
    target: &(PathBuf, String),
    label: &str,
    context: &MergeContext<'_>,
) -> Result<(), MergeError> {
    // Source LINKS edit.
    let contents = staged.get(&source.0).cloned().ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", source.0.display()),
        )
    })?;
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let scan = note_tasks::scan(&contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(source.1.as_str()))
    else {
        return Err(MergeError::new(
            "task_not_found",
            format!("no task `^{}` in `{}`", source.1, source.0.display()),
        ));
    };
    let index = context.note_index.with_additional(staged.keys().cloned());
    let link = dependency_format::canonical_link(
        &target.0, &target.1, &source.0, &index,
    );
    let edit = relation_plan::LinksEdit {
        add: vec![(link.clone(), Some(label.to_string()))],
        ..relation_plan::LinksEdit::default()
    };
    let staged_source = match relation_plan::plan_links_edit(
        &lines,
        &fenced,
        task.line_index,
        &edit,
    ) {
        relation_plan::LinksLinePlan::Noop => contents.clone(),
        relation_plan::LinksLinePlan::Malformed => {
            return Err(MergeError::new(
                "merge_provenance_malformed",
                format!(
                    "cannot record provenance on ^{} in {}: LINKS line is malformed",
                    source.1,
                    source.0.display()
                ),
            ));
        }
        relation_plan::LinksLinePlan::InvalidLabel(_) => {
            return Err(MergeError::new(
                "merge_invalid_label",
                format!("merge label is not a valid relation label: {label}"),
            ));
        }
        relation_plan::LinksLinePlan::Edits { edits, .. } => {
            apply_relation_edits(&contents, &edits)
        }
    };
    // Insert the LINKS line at the canonical slot when the plan is a
    // no-op only because no LINKS line exists yet.
    let staged_source = if staged_source == contents
        && relation_parse::links_children_of(&lines, &fenced, task.line_index)
            .is_empty()
    {
        insert_links_line(&contents, task.line_index, &link, label)?
    } else {
        staged_source
    };
    staged.insert(source.0.clone(), staged_source);

    // Relation mirrors are optional. Merge writes the source LINKS edge even
    // when the caller has disabled target-side LINKED FROM lines; mention
    // mirrors remain owned by the separate mention planner.
    if !context.relation_policy.linked_from_lines {
        return Ok(());
    }

    // Target LINKED FROM edit via the desired-after-delta planner.
    let target_contents = staged.get(&target.0).cloned().ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", target.0.display()),
        )
    })?;
    let target_lines: Vec<&str> = target_contents.lines().collect();
    let target_fenced =
        task_status_hooks::fenced_lines(&target_lines, 0..target_lines.len());
    let target_scan = note_tasks::scan(&target_contents, context.task_settings);
    let Some(target_task) = target_scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(target.1.as_str()))
    else {
        return Err(MergeError::new(
            "task_not_found",
            format!("no task `^{}` in `{}`", target.1, target.0.display()),
        ));
    };
    let existing: Vec<task_relations::RelationEntry> =
        relation_parse::linked_from_children_of(
            &target_lines,
            &target_fenced,
            target_task.line_index,
        )
        .iter()
        .flat_map(|child| match &child.parsed {
            task_relations::LinkedFromLine::Accepted { entries, .. } => {
                entries.clone()
            }
            _ => Vec::new(),
        })
        .collect();
    let source_link = dependency_format::canonical_link(
        &source.0, &source.1, &target.0, &index,
    );
    let mut add = BTreeMap::new();
    add.insert(
        relation_identity(&source.0, &source.1),
        (source_link, Some(label.to_string())),
    );
    let desired_entries = relation_plan::linked_from_desired_after_delta(
        &existing,
        |entry| resolve_relation_identity(&index, &target.0, entry),
        &add,
        &BTreeSet::new(),
    );
    let desired: BTreeSet<(String, Option<String>)> =
        desired_entries.into_iter().collect();
    let staged_target = match relation_plan::plan_linked_from_line(
        &target_lines,
        &target_fenced,
        target_task.line_index,
        &desired,
    ) {
        blocks_mod::BlocksLinePlan::Noop => target_contents.clone(),
        blocks_mod::BlocksLinePlan::Malformed => {
            // Respect mirror settings conservatively: a malformed
            // LINKED FROM line is left alone when mirrors are optional,
            // and refuses only when the policy demands the mirror.
            if context.relation_policy.linked_from_lines {
                return Err(MergeError::new(
                    "merge_provenance_malformed",
                    format!(
                        "cannot record provenance on ^{} in {}: LINKED FROM line is malformed",
                        target.1,
                        target.0.display()
                    ),
                ));
            }
            target_contents.clone()
        }
        blocks_mod::BlocksLinePlan::Edits { edits, .. } => {
            apply_relation_edits(&target_contents, &edits)
        }
    };
    staged.insert(target.0.clone(), staged_target);
    Ok(())
}

fn relation_identity(path: &Path, block: &str) -> String {
    format!("{}#^{block}", path.with_extension("").to_string_lossy())
}

fn resolve_relation_identity(
    index: &NoteIndex,
    owner: &Path,
    entry: &task_relations::RelationEntry,
) -> Option<(String, String)> {
    // Map a stored LINKED FROM entry back to a stable identity for the
    // delta planner: resolve its link target, keep the canonical text.
    if entry.block_id.is_empty() {
        return None;
    }
    let identity = if entry.target.is_empty() {
        format!(
            "{}#^{}",
            owner.with_extension("").to_string_lossy(),
            entry.block_id
        )
    } else {
        match index.resolve_detailed(Some(owner), &entry.target) {
            LinkResolution::Found(path) => {
                format!(
                    "{}#^{}",
                    path.with_extension("").to_string_lossy(),
                    entry.block_id
                )
            }
            LinkResolution::Ambiguous | LinkResolution::Missing => {
                return None;
            }
        }
    };
    Some((identity, entry.canonical_text()))
}

fn apply_relation_edits(
    contents: &str,
    edits: &[relation_plan::RelationEdit],
) -> String {
    let mut note_lines: Vec<String> =
        contents.lines().map(str::to_string).collect();
    let mut inserts: Vec<(usize, String)> = Vec::new();
    let mut replaces: Vec<(usize, String)> = Vec::new();
    let mut removes: Vec<usize> = Vec::new();
    for edit in edits {
        match edit {
            relation_plan::RelationEdit::Insert { line_index, text } => {
                inserts.push((*line_index, text.clone()));
            }
            relation_plan::RelationEdit::Replace { line_index, text } => {
                replaces.push((*line_index, text.clone()));
            }
            relation_plan::RelationEdit::Remove { line_index } => {
                removes.push(*line_index);
            }
        }
    }
    for (line_index, text) in replaces {
        if let Some(slot) = note_lines.get_mut(line_index) {
            *slot = text;
        }
    }
    removes.sort_unstable_by(|a, b| b.cmp(a));
    for line_index in removes {
        if line_index < note_lines.len() {
            note_lines.remove(line_index);
        }
    }
    inserts.sort_by_key(|(line_index, _)| *line_index);
    for (line_index, text) in inserts {
        let at = line_index.min(note_lines.len());
        note_lines.insert(at, text);
    }
    // Preserve the document's line ending conventions.
    let ending = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut updated = note_lines.join(ending);
    if contents.ends_with('\n') && !updated.ends_with('\n') {
        updated.push_str(ending);
    }
    updated
}

fn insert_links_line(
    contents: &str,
    task_line: usize,
    link: &str,
    label: &str,
) -> Result<String, MergeError> {
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = BTreeSet::new();
    let slot = relation_format::relation_insert_slot(
        &lines,
        &fenced,
        task_line,
        relation_format::RelationSlotKind::Links,
    );
    // `format_links_line` renders `label link` from the split entry.
    let canonical = relation_format::format_links_line(
        &slot.indent,
        &[(link.to_string(), Some(label.to_string()))],
    );
    let mut note_lines: Vec<String> =
        contents.lines().map(str::to_string).collect();
    let at = slot.line_index.min(note_lines.len());
    note_lines.insert(at, canonical);
    let ending = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut updated = note_lines.join(ending);
    if contents.ends_with('\n') && !updated.ends_with('\n') {
        updated.push_str(ending);
    }
    Ok(updated)
}

/// Stage section provenance: a Cancel Log entry
/// `*today* — 📥 merged into [[note#Heading]]` on S, creating the log
/// when absent. The same reason is never added twice.
pub(crate) fn stage_section_provenance(
    staged: &mut BTreeMap<PathBuf, String>,
    source: &(PathBuf, String),
    note: &str,
    heading: &str,
    context: &MergeContext<'_>,
) -> Result<(), MergeError> {
    let contents = staged.get(&source.0).cloned().ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", source.0.display()),
        )
    })?;
    let scan = note_tasks::scan(&contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(source.1.as_str()))
    else {
        return Err(MergeError::new(
            "task_not_found",
            format!("no task `^{}` in `{}`", source.1, source.0.display()),
        ));
    };
    let note_path = section_note_path_from_staged(staged, note, context)?;
    let note_target =
        note_path.with_extension("").to_string_lossy().into_owned();
    let reason = section_provenance_reason(&note_target, heading);
    if cancel_log_has_reason(
        &contents,
        task.line_index,
        task.block_end,
        &reason,
    ) {
        return Ok(());
    }
    let edited = task_cancel::line::edit_source_create_log(
        &contents,
        task.line_index,
        context.today,
        &[reason],
    );
    staged.insert(source.0.clone(), edited.content);
    Ok(())
}

fn section_note_path_from_staged(
    staged: &BTreeMap<PathBuf, String>,
    note: &str,
    context: &MergeContext<'_>,
) -> Result<PathBuf, MergeError> {
    let index = context.note_index.with_additional(staged.keys().cloned());
    match index.resolve_detailed(None, note) {
        LinkResolution::Found(path) => Ok(path),
        LinkResolution::Ambiguous => Err(MergeError::new(
            "ambiguous_note",
            format!("`{note}` matches more than one note"),
        )),
        LinkResolution::Missing => Err(MergeError::new(
            "task_not_found",
            format!("no note matches `{note}`"),
        )),
    }
}

/// Compose `plan_task_cancel` for one source over the staged bytes.
/// Graph changes are already staged, so recovery sees repointed
/// dependents and never transiently releases them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cancel_source(
    staged: &mut BTreeMap<PathBuf, String>,
    source: &(PathBuf, String),
    reason: &str,
    context: &MergeContext<'_>,
    daily_override: Option<String>,
) -> Result<task_cancel::TaskCancelPlan, MergeError> {
    let source_text = staged.get(&source.0).cloned().ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", source.0.display()),
        )
    })?;
    let scan = note_tasks::scan(&source_text, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(source.1.as_str()))
    else {
        return Err(MergeError::new(
            "task_not_found",
            format!("no task `^{}` in `{}`", source.1, source.0.display()),
        ));
    };
    let daily_state = match &context.daily_read {
        super::types::DailyNoteReadState::Failed(error) => {
            task_cancel::DailyReadState::Failed(error.clone())
        }
        super::types::DailyNoteReadState::Read(text) => {
            task_cancel::DailyReadState::Present(
                daily_override
                    .as_deref()
                    .or_else(|| {
                        staged.get(&context.daily_path).map(String::as_str)
                    })
                    .unwrap_or(text)
                    .to_string(),
            )
        }
        super::types::DailyNoteReadState::Missing => {
            task_cancel::DailyReadState::Absent
        }
        super::types::DailyNoteReadState::NotRead => {
            if let Some(text) = daily_override
                .as_deref()
                .or_else(|| staged.get(&context.daily_path).map(String::as_str))
            {
                task_cancel::DailyReadState::Present(text.to_string())
            } else {
                task_cancel::DailyReadState::Absent
            }
        }
    };
    // Dependency snapshots: every staged note, so recovery observes
    // the repointed graph rather than the pre-merge bytes.
    let dependency_snapshots: Vec<(PathBuf, String)> = staged
        .iter()
        .map(|(path, text)| (path.clone(), text.clone()))
        .collect();
    let input = task_cancel::TaskCancelInput {
        source_path: source.0.clone(),
        source_block_id: Some(source.1.clone()),
        source_line_index: task.line_index,
        source_text: source_text.clone(),
        source_is_new: false,
        ordered_reasons: vec![reason.to_string()],
        today: context.today,
        note_index: context.note_index,
        tasks_settings: context.task_settings,
        note_settings: context.task_settings,
        daily_path: context.daily_path.clone(),
        daily_state,
        dependency_snapshots,
    };
    task_cancel::plan_task_cancel(input).map_err(|error| match error {
        task_cancel::TaskCancelError::Refused(_) => MergeError::new(
            "merge_source_closed",
            format!("^{} is closed", source.1),
        ),
        task_cancel::TaskCancelError::DailyRead(message) => {
            MergeError::new("merge_daily_read_failed", message)
        }
    })
}

/// Full retirement step used by the integration phase and covered by
/// the MG5–MG9, MG13, and MG16 tests: repoint, provenance, cancel,
/// and reports over the payload's staged map.
///
/// `prepared` carries the payload postimages; `target_identity` is
/// `None` for section destinations. Returns the retired staged map
/// plus per-source reports in source order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_retirement(
    snapshot: &VaultSnapshot,
    prepared: &PreparedMerge,
    request: &MergeRequest,
    target_identity: Option<(PathBuf, String)>,
    context: &MergeContext<'_>,
) -> Result<(BTreeMap<PathBuf, String>, Vec<RetirementReports>), MergeError> {
    let label = normalize_merge_label(&request.label)?;
    // Start from the payload postimages layered over the snapshot so
    // untouched notes remain available to the graph scan.
    let mut staged = snapshot.notes.clone();
    for (path, text) in &prepared.postimages {
        staged.insert(path.clone(), text.clone());
    }
    let sources: BTreeSet<(PathBuf, String)> = prepared
        .sources
        .iter()
        .filter(|source| source.disposition == SourceDisposition::Open)
        .map(|source| (source.path.clone(), source.block_id.clone()))
        .collect();

    // Section destinations refuse any open dependent: there is no task
    // endpoint for them to wait on.
    let dependents = find_open_dependents(&staged, &sources, context);
    let section_heading = match &request.target {
        MergeContainer::Section {
            note: _,
            selector: _,
        } => {
            let container = prepared.container.as_ref().ok_or_else(|| {
                MergeError::new(
                    "merge_container_failed",
                    "section container was not prepared",
                )
            })?;
            match &container.kind {
                super::types::PreparedContainerKind::Section {
                    title, ..
                } => Some(title.clone()),
                _ => None,
            }
        }
        _ => None,
    };
    if section_heading.is_some() && !dependents.is_empty() {
        let first = &dependents[0];
        let message = if let Some(block_id) = &first.block_id {
            format!(
                "“{}” waits on ^{}; merge into a task so it can wait there",
                first.description, block_id,
            )
        } else {
            format!(
                "“{}” in {}:{} depends on a merge source but has no block id; add an id before merging into a section",
                first.description,
                first.path.display(),
                first.line_index + 1,
            )
        };
        return Err(MergeError::new("merge_section_has_dependents", message));
    }

    // Cycle validation over the complete proposed batch graph.
    let mut graph_warnings = Vec::new();
    if let Some(target) = &target_identity {
        let repoints: Vec<((PathBuf, String), (PathBuf, String))> = dependents
            .iter()
            .filter_map(|dependent| {
                let block_id = dependent.block_id.as_ref()?;
                Some((
                    (dependent.path.clone(), block_id.clone()),
                    target.clone(),
                ))
            })
            .filter(|(dependent, _)| dependent != target)
            .collect();
        validate_repoint_cycles(&staged, &repoints, &sources, context)?;
        // Stage graph changes before cancellation recovery.
        let (repointed, warnings) = repoint_dependents(
            &mut staged,
            &dependents,
            &sources,
            target,
            context,
        )?;
        graph_warnings = warnings;
        if !repointed.is_empty() {
            stamp_target_dependency_id(&mut staged, target, context);
        }
    }

    // Provenance + cancellation per open source, in typed order.
    let mut reports = Vec::new();
    for source in &prepared.sources {
        if source.disposition != SourceDisposition::Open {
            reports.push(RetirementReports::default());
            continue;
        }
        let identity = (source.path.clone(), source.block_id.clone());
        let prerequisites = source_prerequisites(&staged, &identity, context);
        let orphans = successor_orphans(&staged, &identity, context);
        let mut links_pruned = 0usize;
        let mut pruned_entries = Vec::new();
        if let Some(target) = &target_identity {
            stage_task_provenance(
                &mut staged,
                &identity,
                target,
                &label,
                context,
            )?;
            let reason = format!(
                "🪢 {label} [[{}#^{}]]",
                target.0.with_extension("").to_string_lossy().into_owned(),
                target.1
            );
            let daily_preimage = staged.get(&context.daily_path).cloned();
            let plan =
                cancel_source(&mut staged, &identity, &reason, context, None)?;
            links_pruned = plan.removed_pomodoro_links;
            pruned_entries = pruned_entry_names(
                daily_preimage.as_deref(),
                &plan.prune.removed_slots,
            );
            staged.insert(identity.0.clone(), plan.source_postimage.clone());
            if let Some(daily) = plan.daily_postimage {
                staged.insert(context.daily_path.clone(), daily);
            }
            for (path, text) in plan.recovery_file_postimages {
                staged.insert(path, text);
            }
        } else if let MergeContainer::Section { note, .. } = &request.target {
            let heading = section_heading.clone().unwrap_or_default();
            // Capture the untouched display form for the log before
            // staging, so the reason matches the resolver's spelling.
            stage_section_provenance(
                &mut staged,
                &identity,
                note,
                &heading,
                context,
            )?;
            // The provenance step already wrote the section Cancel Log
            // entry; cancelling with an empty reason keeps the status
            // change without adding the same reason twice.
            let daily_preimage = staged.get(&context.daily_path).cloned();
            let plan =
                cancel_source(&mut staged, &identity, "", context, None)?;
            links_pruned = plan.removed_pomodoro_links;
            pruned_entries = pruned_entry_names(
                daily_preimage.as_deref(),
                &plan.prune.removed_slots,
            );
            staged.insert(identity.0.clone(), plan.source_postimage.clone());
            if let Some(daily) = plan.daily_postimage {
                staged.insert(context.daily_path.clone(), daily);
            }
            for (path, text) in plan.recovery_file_postimages {
                staged.insert(path, text);
            }
        }
        remove_cancelled_prerequisite_mirrors(&mut staged, &identity, context);
        let repointed_here: Vec<String> = dependents
            .iter()
            .filter(|dependent| {
                // T itself is never repointed even when it waits on S;
                // its edge stays for normal cancel recovery (MG6).
                let dependent_identity = dependent
                    .block_id
                    .as_ref()
                    .map(|block_id| (dependent.path.clone(), block_id.clone()));
                if target_identity.as_ref() == dependent_identity.as_ref() {
                    return false;
                }
                let Some(block_id) = dependent.block_id.as_deref() else {
                    return false;
                };
                // Attribute dependents to the source they waited on by
                // re-resolving the pre-repoint snapshot.
                pre_repoint_waited_on(
                    snapshot,
                    &dependent.path,
                    block_id,
                    &identity,
                    context,
                )
            })
            .map(|dependent| {
                format!(
                    "{}#^{}",
                    dependent.path.display(),
                    dependent.block_id.as_deref().unwrap_or_default()
                )
            })
            .collect();
        reports.push(RetirementReports {
            prerequisites_left: prerequisites,
            successors_orphaned: orphans,
            dependents_repointed: repointed_here,
            links_pruned,
            pruned_entries,
            warnings: std::mem::take(&mut graph_warnings),
        });
    }
    // Idempotency guard for the gesture variants lives in
    // `classify_source`; explicit callers refuse closed sources
    // without a matching edge before reaching this function.
    if request.gesture == MergeGesture::Explicit {
        for source in &prepared.sources {
            if source.disposition != SourceDisposition::Open {
                return Err(closed_source_refusal(
                    snapshot,
                    &source.path,
                    &source.block_id,
                    context.task_settings,
                    context.note_index,
                ));
            }
        }
    }
    Ok((staged, reports))
}

/// Reconciliation assigns an encodable canonical id to a task the first
/// time another task depends on it. Keep that projection in the merge
/// postimage so the production dependency reconciler is already a no-op.
fn stamp_target_dependency_id(
    staged: &mut BTreeMap<PathBuf, String>,
    target: &(PathBuf, String),
    context: &MergeContext<'_>,
) {
    let Some(contents) = staged.get(&target.0).cloned() else {
        return;
    };
    let Some(line_index) = note_tasks::scan(&contents, context.task_settings)
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(target.1.as_str()))
        .map(|task| task.line_index)
    else {
        return;
    };
    let Some(line) = contents.lines().nth(line_index) else {
        return;
    };
    let metadata = task_status_hooks::task_metadata(line, Some(&target.1));
    if metadata.task_id.is_some() {
        return;
    }
    let Ok(canonical) = task_dependencies::dependency_id(&target.0, &target.1)
    else {
        return;
    };
    let mut lines = contents
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let Some(line) = lines.get_mut(line_index) else {
        return;
    };
    let ending = if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let body = line.strip_suffix(ending).unwrap_or(line);
    let updated = task_status_hooks::set_task_fields(
        body,
        Some(&canonical),
        &metadata.depends_on,
    );
    *line = format!("{updated}{ending}");
    staged.insert(target.0.clone(), lines.concat());
}

fn pre_repoint_waited_on(
    snapshot: &VaultSnapshot,
    dependent_path: &Path,
    dependent_block: &str,
    source: &(PathBuf, String),
    context: &MergeContext<'_>,
) -> bool {
    let Some(contents) = snapshot.notes.get(dependent_path) else {
        return false;
    };
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return false;
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let scan = note_tasks::scan(contents, context.task_settings);
    let Some(task) = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(dependent_block))
    else {
        return false;
    };
    let Some(child) =
        dependency_parse::dependency_child_of(&lines, &fenced, task.line_index)
    else {
        return false;
    };
    let dependency_parse::DependencyLine::Accepted { links, .. } =
        &child.parsed
    else {
        return false;
    };
    let index = context
        .note_index
        .with_additional(snapshot.notes.keys().cloned());
    links.iter().any(|link| {
        resolve_dependency_identity(
            &index,
            dependent_path,
            &link.target,
            &link.block_id,
        )
        .as_ref()
            == Some(source)
    })
}

/// The shared overlap-removal rule for MG19, operating on named
/// normalized effect lists. Every key present in more than one list is
/// dropped from all of them with exactly one warning per key.
///
/// Three-list callers (the relations parser) keep their byte-identical
/// warnings; a fourth `merge_labels` list adds the merge-aware copy.
pub(crate) fn overlapping_effect_keys(
    path_display: &str,
    lists: &[(&str, Vec<(String, String)>)],
) -> (BTreeSet<String>, Vec<String>) {
    let mut membership: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for (list_name, entries) in lists {
        for (key, name) in entries {
            membership
                .entry(key.clone())
                .or_default()
                .insert((*list_name).to_string());
            names.entry(key.clone()).or_insert_with(|| name.clone());
            if !order.contains(key) {
                order.push(key.clone());
            }
        }
    }
    let mut drop_keys = BTreeSet::new();
    let mut warnings = Vec::new();
    for key in order {
        let holders = membership.get(&key).cloned().unwrap_or_default();
        if holders.len() <= 1 {
            continue;
        }
        drop_keys.insert(key.clone());
        let name = names.get(&key).cloned().unwrap_or_default();
        let has_merge = holders.contains("merge_labels");
        let only_cancel_complete = holders.len() == 2
            && holders.contains("cancel_labels")
            && holders.contains("complete_labels");
        if has_merge {
            warnings.push(format!(
                "relations: “{name}” is in merge_labels and another effect list in {path_display}; it has no effect"
            ));
        } else if only_cancel_complete {
            warnings.push(format!(
                "relations: “{name}” is in both cancel_labels and complete_labels in {path_display}; it closes nothing"
            ));
        } else {
            warnings.push(format!(
                "relations: “{name}” is in more than one of cancel_labels, complete_labels, successor_labels in {path_display}; it has no effect"
            ));
        }
    }
    (drop_keys, warnings)
}
