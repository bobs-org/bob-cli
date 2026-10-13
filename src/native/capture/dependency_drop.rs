//! Staged dependency removal for capture link gestures.

use std::collections::BTreeSet;

use super::*;
use crate::native::{
    freshness::placement::stamp_fresh,
    task_dependencies::{
        self, format as dependency_format, parse as dependency_parse,
    },
    task_status_hooks,
};

pub(super) enum ResolvedLinkGuard {
    Proceed {
        dropped: Vec<DroppedDependencyJson>,
        task_block_ref: Option<TaskBlockRef>,
        blocks_updates: Vec<BlocksLineUpdateJson>,
    },
    Consume(Box<ConsumePlan>),
}

enum DropAttempt {
    Applied {
        dropped: Vec<DroppedDependencyJson>,
        task_block_ref: TaskBlockRef,
        blocks_updates: Vec<BlocksLineUpdateJson>,
    },
    Refused(ConsumePlan),
}

/// Resolve a guard verdict, applying an eligible dependency drop through
/// the same batch planner used by the rest of the capture gesture.
pub(super) fn resolve_link_guard(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target_rel: &Path,
    block_id: &str,
    verdict: LinkGuardVerdict,
    warnings: &mut Vec<String>,
) -> Result<ResolvedLinkGuard, CaptureError> {
    match verdict {
        LinkGuardVerdict::Link | LinkGuardVerdict::PullForward => {
            Ok(ResolvedLinkGuard::Proceed {
                dropped: Vec::new(),
                task_block_ref: None,
                blocks_updates: Vec::new(),
            })
        }
        LinkGuardVerdict::Consume(plan) => Ok(ResolvedLinkGuard::Consume(plan)),
        LinkGuardVerdict::DropDependencies(plan) => match plan_dependency_drop(
            ctx, planner, bob_dir, target_rel, block_id, *plan, warnings,
        )? {
            DropAttempt::Applied {
                dropped,
                task_block_ref,
                blocks_updates,
            } => Ok(ResolvedLinkGuard::Proceed {
                dropped,
                task_block_ref: Some(task_block_ref),
                blocks_updates,
            }),
            DropAttempt::Refused(plan) => {
                Ok(ResolvedLinkGuard::Consume(Box::new(plan)))
            }
        },
    }
}

fn plan_dependency_drop(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target_rel: &Path,
    block_id: &str,
    mut plan: DropPlan,
    warnings: &mut Vec<String>,
) -> Result<DropAttempt, CaptureError> {
    let absolute = absolute_note(bob_dir, target_rel);
    let contents = current_note_text(planner, &absolute)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let route_label = display_relative(target_rel);
    let task = lookup_staged_task(&scan, &route_label, block_id)?;
    if task.digest != plan.dependent_digest {
        return Err(CaptureError::io(format!(
            "task ^{block_id} in {route_label} changed while planning its dependency drop; refusing to overwrite it"
        )));
    }

    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let managed = managed_children(&lines, &fenced, task.line_index);
    let (managed_at, links) = match managed.as_slice() {
        [(
            line_index,
            dependency_parse::DependencyLine::Accepted { links, .. },
        )] => (*line_index, links),
        _ => {
            plan.fallback.drop_refused = Some("dependency_line");
            return Ok(DropAttempt::Refused(plan.fallback));
        }
    };
    let original_child = lines.get(managed_at).copied().ok_or_else(|| {
        CaptureError::io(format!(
            "dependency-drop invariant failed: Depends-On line disappeared for ^{block_id} in {route_label}"
        ))
    })?;

    let open: BTreeSet<(String, String)> =
        plan.open_prerequisites.iter().cloned().collect();
    let mut dropped = Vec::new();
    let mut dropped_ids = BTreeSet::new();
    let mut kept_links = Vec::new();
    let mut found = BTreeSet::new();

    for (link_index, link) in links.iter().enumerate() {
        let resolved = plan.index.resolve(Some(target_rel), link.target.trim());
        let identity = resolved.as_ref().map(|relative| {
            (display_relative(relative), link.block_id.clone())
        });
        if let (Some(relative), Some(identity)) = (resolved, identity)
            && open.contains(&identity)
        {
            let prerequisite_absolute = absolute_note(bob_dir, &relative);
            let prerequisite_contents =
                current_note_text(planner, &prerequisite_absolute)?;
            let prerequisite_scan =
                note_tasks::scan(&prerequisite_contents, ctx.settings());
            let prerequisite = lookup_staged_task(
                &prerequisite_scan,
                &identity.0,
                &link.block_id,
            )?;
            let prerequisite_line = prerequisite_contents
                .lines()
                .nth(prerequisite.line_index)
                .ok_or_else(|| {
                    CaptureError::io(format!(
                        "dependency-drop invariant failed: prerequisite ^{} disappeared from {}",
                        link.block_id, identity.0
                    ))
                })?;
            let metadata = task_status_hooks::task_metadata(
                prerequisite_line,
                Some(&link.block_id),
            );
            let projected_id = match metadata.task_id {
                Some(id) => id,
                None => {
                    task_dependencies::dependency_id(&relative, &link.block_id)
                        .map_err(|error| CaptureError::io(error.to_string()))?
                }
            };
            dropped_ids.insert(projected_id);
            found.insert(identity);
            dropped.push(DroppedDependencyJson {
                note_path: display_relative(&relative),
                block_id: link.block_id.clone(),
                link: verbatim_link(original_child, link_index, link),
                text: prerequisite.description.clone(),
                status_symbol: prerequisite.status_symbol,
                status_name: prerequisite.status_name.clone(),
            });
        } else {
            kept_links.push(verbatim_link(original_child, link_index, link));
        }
    }

    if found.len() != open.len() {
        let missing = open
            .difference(&found)
            .map(|(note, id)| format!("[[{note}#^{id}]]"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CaptureError::io(format!(
            "dependency-drop invariant failed for ^{block_id} in {route_label}: assessed prerequisites are absent from the accepted Depends-On line ({missing})"
        )));
    }

    let mut note_lines: Vec<String> =
        contents.lines().map(str::to_string).collect();
    if kept_links.is_empty() {
        note_lines.remove(managed_at);
    } else {
        note_lines[managed_at] =
            task_status_hooks::rebuild_child_line(original_child, &kept_links);
    }

    let task_line = note_lines.get(task.line_index).cloned().ok_or_else(|| {
        CaptureError::io(format!(
            "dependency-drop invariant failed: task line disappeared for ^{block_id} in {route_label}"
        ))
    })?;
    let metadata = task_status_hooks::task_metadata(&task_line, Some(block_id));
    let kept_field_ids: Vec<String> = metadata
        .depends_on
        .into_iter()
        .filter(|id| !dropped_ids.contains(id))
        .collect();
    let without_dropped_fields = task_status_hooks::set_task_fields(
        &task_line,
        metadata.task_id.as_deref(),
        &kept_field_ids,
    );
    let stamp = stamp_fresh(&without_dropped_fields, ctx.today);
    note_lines[task.line_index] = if stamp.refused.is_none() {
        stamp.line
    } else {
        without_dropped_fields
    };

    let ending = dependency_format::note_line_ending(&contents);
    let final_newline = dependency_format::has_final_newline(&contents);
    let mut updated = note_lines.join(ending);
    if final_newline && !note_lines.is_empty() {
        updated.push_str(ending);
    }
    planner.stage(&absolute, updated)?;

    let route = target_rel
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&route_label)
        .to_string();
    // BLOCKS mirror (§14.7): stage the dependent's removal from each
    // dropped prerequisite's line in the same batch. Prerequisite task
    // lines are still never edited; closed, archive, and
    // previous-daily owners are left for reconcile.
    let mut blocks_updates = Vec::new();
    let dependent = (route_label.clone(), block_id.to_string());
    for (note, owner) in &found {
        let owner_relative = PathBuf::from(note);
        if let Some((update, _)) = stage_blocks_delta(
            ctx,
            planner,
            bob_dir,
            &plan.index,
            &owner_relative,
            owner,
            &dependent,
            BlocksDelta::Remove,
            warnings,
        )? {
            blocks_updates.push(update);
        }
    }
    Ok(DropAttempt::Applied {
        dropped,
        task_block_ref: TaskBlockRef {
            target: absolute,
            relative_target: route_label,
            route,
            line: task.line_index,
            block_id: Some(block_id.to_string()),
            role: TaskBlockRole::DependencyDropped,
        },
        blocks_updates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        fs,
    };
    use tempfile::tempdir;

    fn candidate(
        dependent_line: &str,
        open_prerequisites: &[(&str, &str)],
    ) -> DropPlan {
        let identities: Vec<(String, String)> = open_prerequisites
            .iter()
            .map(|(note, block)| ((*note).to_string(), (*block).to_string()))
            .collect();
        let graph = DependencyGraph {
            edges: BTreeMap::new(),
            open: HashMap::new(),
            scheduled: HashMap::new(),
            status: HashMap::new(),
            never_raise: BTreeSet::new(),
            project_residents: BTreeMap::new(),
        };
        DropPlan {
            open_prerequisites: identities.clone(),
            dependent_digest: note_tasks::line_digest(dependent_line),
            index: crate::native::vault_links::NoteIndex::from_paths([
                PathBuf::from("cash.md"),
                PathBuf::from("job.md"),
                PathBuf::from("done.md"),
            ]),
            fallback: ConsumePlan {
                reasons: Vec::new(),
                seeds: identities,
                lane: 1,
                graph,
                drop_refused: None,
            },
        }
    }

    fn apply(
        vault: &Path,
        target: &Path,
        block_id: &str,
        plan: DropPlan,
    ) -> DropAttempt {
        let context = DependencyContext::new(
            vault,
            NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(),
        );
        let mut planner = CaptureBatchPlanner::default();
        let attempt = plan_dependency_drop(
            &context,
            &mut planner,
            vault,
            target,
            block_id,
            plan,
            &mut Vec::new(),
        )
        .expect("plan dependency drop");
        if let DropAttempt::Applied { .. } = &attempt {
            let staged = planner
                .current_contents(&vault.join(target))
                .expect("read staged dependent")
                .expect("staged dependent");
            fs::write(vault.join("actual.md"), staged)
                .expect("persist staged text for the test");
        }
        attempt
    }

    #[test]
    fn partial_drop_keeps_verbatim_history_and_unresolved_links_and_field_ids()
    {
        let dir = tempdir().expect("temp vault");
        let vault = dir.path();
        let dependent = concat!(
            "- [?] #task Work [id:: cash__work] [dependsOn:: job__prep, done__done, missing__ghost] ^work\n",
            "  - ⛓️ **DEPENDS ON:** [[job#^prep]] • [[done#^done|Done alias]] • [[missing#^ghost]]\n",
        );
        let task_line = dependent.lines().next().unwrap();
        fs::write(vault.join("cash.md"), dependent).expect("write dependent");
        fs::write(vault.join("job.md"), "- [ ] #task Prep ^prep\n")
            .expect("write open prerequisite");
        fs::write(vault.join("done.md"), "- [x] #task Done ^done\n")
            .expect("write closed prerequisite");
        let plan = candidate(task_line, &[("job.md", "prep")]);

        let attempt = apply(vault, Path::new("cash.md"), "work", plan);
        let DropAttempt::Applied { dropped, .. } = attempt else {
            panic!("expected an applied drop")
        };
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].link, "[[job#^prep]]");
        assert_eq!(
            fs::read_to_string(vault.join("actual.md")).unwrap(),
            concat!(
                "- [?] #task Work [fresh:: 2026-10-12] [id:: cash__work] [dependsOn:: done__done, missing__ghost] ^work\n",
                "  - ⛓️ **DEPENDS ON:** [[done#^done|Done alias]] • [[missing#^ghost]]\n",
            )
        );
    }

    #[test]
    fn last_dependency_line_and_field_are_removed_with_crlf_and_final_newline_preserved(
    ) {
        for (suffix, final_newline) in [("\r\n", true), ("", false)] {
            let dir = tempdir().expect("temp vault");
            let vault = dir.path();
            let dependent = format!(
                "- [?] #task Work [dependsOn:: job__prep] ^work\r\n  - ⛓️ **DEPENDS ON:** [[job#^prep]]{suffix}"
            );
            let task_line = dependent.lines().next().unwrap().to_string();
            fs::write(vault.join("cash.md"), &dependent)
                .expect("write dependent");
            fs::write(vault.join("job.md"), "- [ ] #task Prep ^prep\n")
                .expect("write prerequisite");
            let plan = candidate(&task_line, &[("job.md", "prep")]);

            let attempt = apply(vault, Path::new("cash.md"), "work", plan);
            assert!(matches!(attempt, DropAttempt::Applied { .. }));
            let updated = fs::read_to_string(vault.join("actual.md"))
                .expect("read updated note");
            assert!(!updated.contains("DEPENDS ON"), "{updated:?}");
            assert!(!updated.contains("dependsOn::"), "{updated:?}");
            assert!(!updated.contains('\n') || updated.contains("\r\n"));
            assert_eq!(updated.ends_with("\r\n"), final_newline);
        }
    }

    #[test]
    fn duplicate_or_malformed_managed_lines_refuse_without_staging() {
        let cases = [
            concat!(
                "- [?] #task Work ^work\n",
                "  - ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
                "  - ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
            ),
            concat!(
                "- [?] #task Work ^work\n",
                "  - ⛓️ **DEPENDS ON:** [[job#^prep]] trailing\n",
            ),
        ];
        for dependent in cases {
            let dir = tempdir().expect("temp vault");
            let vault = dir.path();
            let task_line = dependent.lines().next().unwrap();
            fs::write(vault.join("cash.md"), dependent)
                .expect("write dependent");
            fs::write(vault.join("job.md"), "- [ ] #task Prep ^prep\n")
                .expect("write prerequisite");
            let context = DependencyContext::new(
                vault,
                NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(),
            );
            let mut planner = CaptureBatchPlanner::default();
            let attempt = plan_dependency_drop(
                &context,
                &mut planner,
                vault,
                Path::new("cash.md"),
                "work",
                candidate(task_line, &[("job.md", "prep")]),
                &mut Vec::new(),
            )
            .expect("plan fallback consumption");
            let DropAttempt::Refused(plan) = attempt else {
                panic!("expected G5 refusal")
            };
            assert_eq!(plan.drop_refused, Some("dependency_line"));
            assert!(planner.files.iter().all(|file| {
                file.present == file.existed && file.current == file.original
            }));
            assert_eq!(
                fs::read_to_string(vault.join("cash.md")).unwrap(),
                dependent
            );
        }
    }
}
