//! Batch-scoped relation-source snapshot assembly for capture.
//!
//! The caller runs the cheap label / kill-switch / event gate before this
//! module touches the note catalog. Once admitted, the shared byte pass
//! supplies the relation source view and the dependency proof independently;
//! staged text is overlaid for each gesture and target notes are loaded only
//! through `CaptureBatchPlanner`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    batch::CaptureBatchPlanner,
    dependencies::{staged_note_index, under_archive_dir, DependencyContext},
};
use crate::native::{
    config::relations::CancelLabel,
    relation_labels::relation_label_key,
    task_complete::relation_successors::{
        RelationEvent, RelationSuccessorInput,
    },
    task_relations::{
        parse::links_children_of, resolve::resolve_relation_target_path,
        LinksLine,
    },
    task_status_hooks::{self, TasksSettings},
    vault_links::NoteIndex,
};

/// The source and dependency note sets needed by one relation-enabled
/// successor plan. Dependency proof is separate from the caller's normal
/// recovery completeness: carrying these notes does not enable recovery when
/// that consumer did not request it.
pub(super) struct RelationSourceRead {
    pub(super) input: RelationSuccessorInput,
    pub(super) source_notes: Vec<(PathBuf, String)>,
    pub(super) dependency_notes: Vec<(PathBuf, String)>,
    pub(super) target_notes: Vec<(PathBuf, String)>,
}

/// Assemble relation input after the cheap gate. `task_complete` passes the
/// result to `plan_successors`; Pomodoro parked events stay on the parks
/// phase.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_relation_sources(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    events: &[RelationEvent],
    parked: &[RelationEvent],
    existing_note_texts: &[(PathBuf, String)],
    tasks_settings: &TasksSettings,
) -> Result<Option<RelationSourceRead>, super::CaptureError> {
    let labels = ctx.successor_labels().to_vec();
    if labels.is_empty() {
        return Ok(None);
    }
    if !ctx.link_unblocked() {
        return Ok(None);
    }
    let trigger_events: Vec<&RelationEvent> =
        events.iter().chain(parked.iter()).collect();
    if trigger_events.is_empty() {
        return Ok(None);
    }
    if trigger_events
        .iter()
        .all(|event| event.target.block_id.is_empty())
    {
        return Ok(None);
    }

    // The catalog walk and all note reads begin after G0-G3. If dependency
    // recovery already requested the snapshot earlier in this batch, this
    // derives a relation view from its retained raw bytes without rereading.
    let relation_snapshot = ctx.relation_sources_snapshot();
    let dependency_snapshot = ctx.dependents_snapshot();
    let staged = staged_markdown_overlay(bob_dir, planner);
    let relation_snapshot_complete =
        relation_snapshot.snapshot_complete(&staged);
    let dependency_snapshot_complete =
        dependency_snapshot.snapshot_complete(&staged);
    let dependency_notes = dependency_snapshot.overlaid(staged.clone());

    let mut source_notes = Vec::new();
    let mut target_refs = BTreeSet::<(PathBuf, String)>::new();
    let mut unavailable_targets = BTreeSet::new();

    if relation_snapshot_complete {
        let staged_index = staged_note_index(ctx, planner, bob_dir);
        let event_targets: BTreeSet<(PathBuf, String)> = trigger_events
            .iter()
            .filter(|event| !event.target.block_id.is_empty())
            .map(|event| {
                (
                    event.target.note_path.clone(),
                    event.target.block_id.clone(),
                )
            })
            .collect();
        let event_ids: BTreeSet<&str> = trigger_events
            .iter()
            .filter(|event| !event.target.block_id.is_empty())
            .map(|event| event.target.block_id.as_str())
            .collect();

        for (source_path, contents) in relation_snapshot.overlaid(staged) {
            // Cheap per-gesture token filter before parsing a LINKS note.
            // A false positive is harmless; a shared block-id token cannot
            // be missed by the parser's accepted `#^id` link forms.
            if !contains_event_block_token(&contents, &event_ids) {
                continue;
            }
            let (matched, targets) = source_targets_for_events(
                &source_path,
                &contents,
                &labels,
                &event_targets,
                &staged_index,
                tasks_settings,
            );
            if !matched {
                continue;
            }
            target_refs.extend(targets);
            source_notes.push((source_path, contents));
        }
    }

    let source_texts: BTreeMap<PathBuf, String> =
        source_notes.iter().cloned().collect();
    for (relative, contents) in &source_notes {
        planner.current_contents_from_snapshot(
            &bob_dir.join(relative),
            Some(contents.clone()),
        )?;
    }
    let existing_paths: BTreeSet<PathBuf> = existing_note_texts
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    let mut target_notes = BTreeMap::<PathBuf, String>::new();

    // Event targets are explicit core inputs. Their note text is also made
    // available to the core's in-memory reader, unless the caller already
    // staged it or a matched source note supplies the same note.
    let event_paths: BTreeSet<PathBuf> = event_paths_for_notes(&trigger_events);
    for path in event_paths {
        load_target_note(
            ctx,
            &path,
            bob_dir,
            planner,
            &source_texts,
            &existing_paths,
            &mut target_notes,
            &mut unavailable_targets,
        )?;
    }
    for (path, _) in &target_refs {
        load_target_note(
            ctx,
            path,
            bob_dir,
            planner,
            &source_texts,
            &existing_paths,
            &mut target_notes,
            &mut unavailable_targets,
        )?;
    }

    Ok(Some(RelationSourceRead {
        input: RelationSuccessorInput {
            labels,
            parked: parked.to_vec(),
            relation_snapshot_complete,
            dependency_snapshot_complete,
            previous_daily: ctx
                .previous_daily()
                .and_then(|path| path.strip_prefix(bob_dir).ok())
                .map(Path::to_path_buf),
            unavailable_targets,
        },
        source_notes,
        dependency_notes,
        target_notes: target_notes.into_iter().collect(),
    }))
}

fn staged_markdown_overlay(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
) -> BTreeMap<PathBuf, Option<String>> {
    planner
        .staged_snapshot()
        .into_iter()
        .filter_map(|(absolute, contents)| {
            let relative = absolute.strip_prefix(bob_dir).ok()?.to_path_buf();
            relative
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
                .then_some((relative, contents))
        })
        .collect()
}

fn event_paths_for_notes(events: &[&RelationEvent]) -> BTreeSet<PathBuf> {
    events
        .iter()
        .filter(|event| !event.target.block_id.is_empty())
        .map(|event| event.target.note_path.clone())
        .collect()
}

fn contains_event_block_token(
    contents: &str,
    event_ids: &BTreeSet<&str>,
) -> bool {
    event_ids.iter().any(|id| {
        let mut token = String::with_capacity(id.len() + 1);
        token.push('^');
        token.push_str(id);
        memchr::memmem::find(contents.as_bytes(), token.as_bytes()).is_some()
    })
}

fn source_targets_for_events(
    source_path: &Path,
    contents: &str,
    labels: &[CancelLabel],
    event_targets: &BTreeSet<(PathBuf, String)>,
    index: &NoteIndex,
    tasks_settings: &TasksSettings,
) -> (bool, BTreeSet<(PathBuf, String)>) {
    let label_keys: BTreeSet<&str> =
        labels.iter().map(|label| label.key.as_str()).collect();
    let lines = task_status_hooks::logical_lines(contents);
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let tasks = task_status_hooks::parse_tasks(contents, tasks_settings);
    let mut matched = false;
    let mut targets = BTreeSet::new();

    for task in &tasks {
        let mut task_matched = false;
        let mut task_targets = BTreeSet::new();
        for child in links_children_of(&lines, &fenced, task.line_index) {
            let LinksLine::Accepted { entries, .. } = child.parsed else {
                continue;
            };
            for entry in entries {
                let Some(label) = entry.label.as_deref() else {
                    continue;
                };
                if !label_keys.contains(relation_label_key(label).as_str()) {
                    continue;
                }
                let Some(target_path) = resolve_relation_target_path(
                    source_path,
                    &entry.target,
                    index,
                    under_archive_dir,
                ) else {
                    continue;
                };
                if task.block_id.as_deref() == Some(entry.block_id.as_str())
                    && target_path == source_path
                {
                    continue;
                }
                let target = (target_path.clone(), entry.block_id.clone());
                if event_targets.contains(&target) {
                    task_matched = true;
                }
                task_targets.insert(target);
            }
        }
        if task_matched {
            matched = true;
            targets.extend(task_targets);
        }
    }
    (matched, targets)
}

#[allow(clippy::too_many_arguments)]
fn load_target_note(
    ctx: &DependencyContext,
    relative: &Path,
    bob_dir: &Path,
    planner: &mut CaptureBatchPlanner,
    source_texts: &BTreeMap<PathBuf, String>,
    existing_paths: &BTreeSet<PathBuf>,
    target_notes: &mut BTreeMap<PathBuf, String>,
    unavailable_targets: &mut BTreeSet<PathBuf>,
) -> Result<(), super::CaptureError> {
    if source_texts.contains_key(relative)
        || existing_paths.contains(relative)
        || target_notes.contains_key(relative)
    {
        return Ok(());
    }
    let absolute = bob_dir.join(relative);
    match planner.current_contents_from_snapshot(
        &absolute,
        ctx.cached_note_text(relative),
    ) {
        Ok(Some(contents)) => {
            target_notes.insert(relative.to_path_buf(), contents);
        }
        Ok(None) | Err(_) => {
            // A missing/unreadable resolved note makes R6 uncertain, but
            // does not turn a successfully planned close into an error.
            unavailable_targets.insert(relative.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use chrono::NaiveDate;

    use crate::native::{
        capture::dependencies::{display_relative, DependentsSnapshot},
        task_complete::relation_successors::{
            RelationEventKind, RelationTarget,
        },
    };

    fn write_note(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("note parent"))
            .expect("create note directory");
        fs::write(path, contents).expect("write note");
    }

    fn context(
        root: &Path,
        labels: &[&str],
        link_unblocked: bool,
    ) -> DependencyContext {
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).expect("date");
        let ctx = DependencyContext::new(root, today);
        ctx.set_successor_labels_for_test(
            labels
                .iter()
                .map(|name| CancelLabel {
                    key: relation_label_key(name),
                    name: (*name).to_string(),
                })
                .collect(),
        );
        ctx.set_link_unblocked_for_test(link_unblocked);
        ctx
    }

    fn event(note: &str, block_id: &str) -> RelationEvent {
        RelationEvent {
            target: RelationTarget {
                note_path: PathBuf::from(note),
                block_id: block_id.to_string(),
                text: "Target".to_string(),
            },
            kind: RelationEventKind::Done,
            slot: None,
        }
    }

    fn parked_event(note: &str, block_id: &str) -> RelationEvent {
        RelationEvent {
            kind: RelationEventKind::Parked,
            ..event(note, block_id)
        }
    }

    fn settings(root: &Path) -> TasksSettings {
        task_status_hooks::read_tasks_settings(root)
    }

    fn collect(
        ctx: &DependencyContext,
        planner: &mut CaptureBatchPlanner,
        root: &Path,
        events: &[RelationEvent],
    ) -> RelationSourceRead {
        collect_relation_sources(
            ctx,
            planner,
            root,
            events,
            &[],
            &[],
            &settings(root),
        )
        .expect("collect relation sources")
        .expect("candidate passes gate")
    }

    #[test]
    fn either_consumer_order_walks_and_reads_the_catalog_once() {
        for dependency_first in [true, false] {
            let vault = tempfile::tempdir().expect("vault");
            let root = vault.path();
            write_note(
                root,
                "source.md",
                "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
            );
            write_note(root, "target.md", "- [ ] #task Target ^t\n");
            write_note(root, "plain.md", "plain text\n");
            let ctx = context(root, &["starts after"], true);
            let mut planner = CaptureBatchPlanner::default();
            let events = [event("target.md", "t")];

            if dependency_first {
                let _ = ctx.dependents_snapshot();
                let _ = collect(&ctx, &mut planner, root, &events);
            } else {
                let _ = collect(&ctx, &mut planner, root, &events);
                let _ = ctx.dependents_snapshot();
            }

            assert_eq!(ctx.catalog_walk_count(), 1);
            let reads = ctx.snapshot_read_counts();
            assert_eq!(reads.get(Path::new("source.md")), Some(&1));
            assert_eq!(reads.get(Path::new("target.md")), Some(&1));
            assert_eq!(reads.get(Path::new("plain.md")), Some(&1));
            assert!(reads.values().all(|count| *count == 1));
        }
    }

    #[test]
    fn cheap_gate_rejects_without_catalog_or_source_reads() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "source.md",
            "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
        );
        let cases = [
            (context(root, &[], true), vec![event("target.md", "t")]),
            (
                context(root, &["starts after"], false),
                vec![event("target.md", "t")],
            ),
            (context(root, &["starts after"], true), vec![]),
            (
                context(root, &["starts after"], true),
                vec![event("target.md", "")],
            ),
        ];
        for (ctx, events) in cases {
            let mut planner = CaptureBatchPlanner::default();
            assert!(collect_relation_sources(
                &ctx,
                &mut planner,
                root,
                &events,
                &[],
                &[],
                &settings(root),
            )
            .expect("gate")
            .is_none());
            assert_eq!(ctx.catalog_walk_count(), 0);
            assert!(ctx.snapshot_read_counts().is_empty());
        }
    }

    #[test]
    fn parked_only_event_selects_sources_and_loads_its_target() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "source.md",
            "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
        );
        write_note(root, "target.md", "- [/] #task Target ^t\n");
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let parked = [parked_event("target.md", "t")];
        let result = collect_relation_sources(
            &ctx,
            &mut planner,
            root,
            &[],
            &parked,
            &[],
            &settings(root),
        )
        .expect("collect parked relation sources")
        .expect("parked event passes the gate");

        assert_eq!(result.source_notes.len(), 1);
        assert_eq!(result.source_notes[0].0, PathBuf::from("source.md"));
        assert_eq!(result.input.parked, vec![parked_event("target.md", "t")]);
        assert_eq!(
            result
                .target_notes
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([PathBuf::from("target.md")])
        );
    }

    #[test]
    fn dependency_snapshot_does_not_surface_unconsulted_relation_warning() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(root, "note.md", "- [ ] #task Task [id:: task] ^t\n");
        let ctx = DependencyContext::new(
            root,
            NaiveDate::from_ymd_opt(2026, 11, 2).expect("date"),
        );
        ctx.set_close_policy_warning_for_test();
        ctx.set_link_unblocked_for_test(true);

        let _ = ctx.dependents_snapshot();
        assert!(ctx.cancel_labels_warning().is_none());
        let _ = ctx.successor_labels();
        assert_eq!(
            ctx.cancel_labels_warning().as_deref(),
            Some("invalid relations config")
        );
    }

    #[test]
    fn source_snapshot_uses_shared_parser_resolver_and_narrow_target_reads() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "source.md",
            concat!(
                "- [ ] #task Source [dependsOn:: dep] ^s\n",
                "  - 🪢 **LINKS:** starts after [[target#^t]] • starts after [[other#^o]]\n",
            ),
        );
        write_note(
            root,
            "target.md",
            "- [x] #task Done target [id:: target] ^t\n",
        );
        write_note(
            root,
            "dep.md",
            "- [ ] #task Open prerequisite [id:: dep] ^d\n",
        );
        write_note(root, "other.md", "- [ ] #task Waiting target ^o\n");
        write_note(root, "unrelated.md", "- [ ] #task Unrelated ^u\n");
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let result =
            collect(&ctx, &mut planner, root, &[event("target.md", "t")]);

        assert!(result.input.relation_snapshot_complete);
        assert!(result.input.dependency_snapshot_complete);
        assert_eq!(
            result
                .source_notes
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>(),
            vec![PathBuf::from("source.md")]
        );
        assert_eq!(
            result
                .dependency_notes
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                PathBuf::from("dep.md"),
                PathBuf::from("source.md"),
                PathBuf::from("target.md"),
            ])
        );
        let target_paths: BTreeSet<_> = result
            .target_notes
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(
            target_paths,
            BTreeSet::from([
                PathBuf::from("target.md"),
                PathBuf::from("other.md")
            ])
        );
        assert!(planner.by_path.contains_key(&root.join("target.md")));
        assert!(!result
            .input
            .unavailable_targets
            .contains(Path::new("unrelated.md")));
    }

    #[test]
    fn staged_overlay_adds_edits_and_deletions_and_masks_read_failures() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(root, "event.md", "- [ ] #task Target ^t\n");
        write_note(root, "source.md", "- [ ] #task Source ^s\n");
        write_note(root, "failed.md", "- [ ] #task Broken ^b\n");
        let ctx = context(root, &["starts after"], true);
        let _ = ctx.dependents_snapshot();
        let mut planner = CaptureBatchPlanner::default();
        planner
            .stage(
                &root.join("source.md"),
                "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[event#^t]]\n"
                    .to_string(),
            )
            .expect("stage edit");
        planner
            .stage(
                &root.join("new.md"),
                "- [ ] #task New ^n\n  - 🪢 **LINKS:** starts after [[event#^t]]\n"
                    .to_string(),
            )
            .expect("stage addition");
        let failed_path = root.join("failed.md");
        let _ = planner
            .current_contents(&failed_path)
            .expect("load failed note");
        let failed_index = planner.by_path[&failed_path];
        planner.files[failed_index].present = false;

        let staged = staged_markdown_overlay(root, &planner);
        let relation = ctx.relation_sources_snapshot();
        assert!(relation.snapshot_complete(&staged));
        let overlaid = relation.overlaid(staged.clone());
        assert!(overlaid
            .iter()
            .any(|(path, _)| path == Path::new("source.md")));
        assert!(overlaid.iter().any(|(path, _)| path == Path::new("new.md")));
        assert!(!overlaid
            .iter()
            .any(|(path, _)| path == Path::new("failed.md")));

        // Relevant invalid UTF-8 taints only its predicate.
        let dep_invalid = BTreeMap::from([(
            PathBuf::from("dep-invalid.md"),
            b"id::\xff".to_vec(),
        )]);
        let rel_invalid = BTreeMap::from([(
            PathBuf::from("rel-invalid.md"),
            b"LINKS\xff".to_vec(),
        )]);
        let reads = Vec::new();
        let dep = DependentsSnapshot::from_raw(
            true,
            &dep_invalid,
            &reads,
            super::super::dependencies::SnapshotKind::Dependencies,
        );
        let rel = DependentsSnapshot::from_raw(
            true,
            &dep_invalid,
            &reads,
            super::super::dependencies::SnapshotKind::Relations,
        );
        assert!(!dep.snapshot_complete(&BTreeMap::new()));
        assert!(rel.snapshot_complete(&BTreeMap::new()));
        assert!(dep.snapshot_complete(&BTreeMap::from([(
            PathBuf::from("dep-invalid.md"),
            Some("- [ ] #task Replaced ^replacement\n".to_string()),
        )])));
        let rel = DependentsSnapshot::from_raw(
            true,
            &rel_invalid,
            &reads,
            super::super::dependencies::SnapshotKind::Relations,
        );
        let dep = DependentsSnapshot::from_raw(
            true,
            &rel_invalid,
            &reads,
            super::super::dependencies::SnapshotKind::Dependencies,
        );
        assert!(!rel.snapshot_complete(&BTreeMap::new()));
        assert!(dep.snapshot_complete(&BTreeMap::new()));
        assert!(rel.snapshot_complete(&BTreeMap::from([(
            PathBuf::from("rel-invalid.md"),
            None,
        )])));
        let read_failed = DependentsSnapshot::from_raw(
            true,
            &BTreeMap::new(),
            &[PathBuf::from("failed.md")],
            super::super::dependencies::SnapshotKind::Relations,
        );
        assert!(!read_failed.snapshot_complete(&BTreeMap::new()));
        assert!(read_failed.snapshot_complete(&BTreeMap::from([(
            PathBuf::from("failed.md"),
            None
        )])));
    }

    #[test]
    fn relation_archive_rules_exclude_done_directories_but_keep_done_md() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "done/archive.md",
            "- [ ] #task Archived source ^a\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
        );
        write_note(
            root,
            "done.md",
            "- [ ] #task Named done source ^d\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
        );
        write_note(root, "target.md", "- [x] #task Target ^t\n");
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let result =
            collect(&ctx, &mut planner, root, &[event("target.md", "t")]);
        assert_eq!(result.source_notes.len(), 1);
        assert_eq!(result.source_notes[0].0, PathBuf::from("done.md"));
    }

    #[test]
    fn a_fresh_context_observes_new_disk_edges_without_a_persistent_cache() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(root, "target.md", "- [x] #task Target ^t\n");
        let old = context(root, &["starts after"], true);
        let _ = old.dependents_snapshot();
        write_note(
            root,
            "source.md",
            "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
        );
        let fresh = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let result =
            collect(&fresh, &mut planner, root, &[event("target.md", "t")]);
        assert_eq!(result.source_notes.len(), 1);
        assert_eq!(display_relative(&result.source_notes[0].0), "source.md");
    }
}
