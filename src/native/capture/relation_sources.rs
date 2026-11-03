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
    dependencies::{under_archive_dir, DependencyContext, DependentsSnapshot},
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
/// phase. The caller supplies its already-built staged index so this
/// function never rebuilds it.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_relation_sources(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    events: &[RelationEvent],
    parked: &[RelationEvent],
    existing_note_texts: &[(PathBuf, String)],
    tasks_settings: &TasksSettings,
    note_index: &NoteIndex,
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

    // Remember event tokens before the shared byte pass so workers keep
    // only LINKS notes that can match this close. Callers that snapshot
    // earlier must remember the same IDs first; a later call is ignored.
    ctx.remember_relation_event_ids(
        trigger_events
            .iter()
            .map(|event| event.target.block_id.as_str()),
    );

    // The catalog walk and all note reads begin after G0-G3. If dependency
    // recovery already requested the snapshot earlier in this batch, this
    // derives a relation view from the shared worker-classified texts
    // without rereading.
    let relation_snapshot = ctx.relation_sources_snapshot();
    let dependency_snapshot = ctx.dependents_snapshot();
    let staged = staged_markdown_overlay(bob_dir, planner);
    let relation_snapshot_complete =
        relation_snapshot.snapshot_complete(&staged);
    let dependency_snapshot_complete =
        dependency_snapshot.snapshot_complete(&staged);

    let mut source_notes = Vec::new();
    let mut target_refs = BTreeSet::<(PathBuf, String)>::new();
    let mut unavailable_targets = BTreeSet::new();

    if relation_snapshot_complete {
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

        // Borrow the snapshot and clone only token-matching notes. Staged
        // text supersedes disk via the shared overlay helper.
        let candidates = relation_snapshot
            .overlaid_matching(&staged, |_, contents| {
                contains_event_block_token(contents, &event_ids)
            });
        for (source_path, contents) in candidates {
            let (matched, targets) = source_targets_for_events(
                &source_path,
                &contents,
                &labels,
                &event_targets,
                note_index,
                tasks_settings,
            );
            if !matched {
                continue;
            }
            target_refs.extend(targets);
            source_notes.push((source_path, contents));
        }
    }

    // Dependency proof is only needed when at least one source matched.
    // With zero matched sources the engine needs no dependency texts.
    // Matched sources need only their own prerequisite / identity notes,
    // never a clone of the whole dependents snapshot.
    let dependency_notes = if source_notes.is_empty() {
        Vec::new()
    } else {
        dependency_proof_notes(
            dependency_snapshot,
            &staged,
            &source_notes,
            tasks_settings,
        )
    };

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
            relation_snapshot,
            dependency_snapshot,
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
            relation_snapshot,
            dependency_snapshot,
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

/// Notes the engine needs to prove a matched source's own R4 state:
/// the source note itself (when it carries dependency markers) plus
/// notes that hold `[id::]` / `(id::)` for its `dependsOn` IDs. Unrelated
/// `id::` notes stay out so `plan_successors` does not parse the whole
/// dependents snapshot on a relation-only close.
fn dependency_proof_notes(
    snapshot: &DependentsSnapshot,
    staged: &BTreeMap<PathBuf, Option<String>>,
    source_notes: &[(PathBuf, String)],
    tasks_settings: &TasksSettings,
) -> Vec<(PathBuf, String)> {
    let source_paths: BTreeSet<&Path> = source_notes
        .iter()
        .map(|(path, _)| path.as_path())
        .collect();
    let mut ids = BTreeSet::new();
    for (_, contents) in source_notes {
        for task in task_status_hooks::parse_tasks(contents, tasks_settings) {
            ids.extend(task.depends_on);
        }
    }
    let needles: Vec<Vec<u8>> = ids
        .iter()
        .map(|id| {
            let mut needle = Vec::from(&b"id:: "[..]);
            needle.extend_from_slice(id.as_bytes());
            needle
        })
        .collect();
    snapshot.overlaid_matching(staged, |path, contents| {
        if source_paths.contains(path) {
            return true;
        }
        let bytes = contents.as_bytes();
        needles
            .iter()
            .any(|needle| memchr::memmem::find(bytes, needle).is_some())
    })
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
    relation_snapshot: &DependentsSnapshot,
    dependency_snapshot: &DependentsSnapshot,
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
    // Reuse snapshot text when this note is a member, so an already-scanned
    // path is never read twice. Marker-negative notes fall through to a
    // single narrow planner read and still reach R6.
    if let Some(snapshot_text) = relation_snapshot
        .get(relative)
        .or_else(|| dependency_snapshot.get(relative))
    {
        match planner.current_contents_from_snapshot(
            &absolute,
            Some(snapshot_text.to_string()),
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
        return Ok(());
    }
    match planner.current_contents(&absolute) {
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
        capture::dependencies::display_relative,
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
        let index =
            super::super::dependencies::staged_note_index(ctx, planner, root);
        collect_relation_sources(
            ctx,
            planner,
            root,
            events,
            &[],
            &[],
            &settings(root),
            &index,
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
            // The cheap G0-G3 gate rejects before the snapshot; callers
            // only build their staged index when a candidate may pass, so
            // pass an empty index and prove no catalog or byte-pass reads.
            let index = NoteIndex::default();
            assert!(collect_relation_sources(
                &ctx,
                &mut planner,
                root,
                &events,
                &[],
                &[],
                &settings(root),
                &index,
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
        let index =
            super::super::dependencies::staged_note_index(&ctx, &planner, root);
        let result = collect_relation_sources(
            &ctx,
            &mut planner,
            root,
            &[],
            &parked,
            &[],
            &settings(root),
            &index,
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

        // Worker classification keeps per-predicate failure sets: an
        // invalid-UTF-8 note matching only one marker taints only that
        // snapshot. Covering the tainted path with a staged deletion
        // restores completeness even while the other predicate's invalid
        // note stays uncovered.
        {
            let invalid_vault = tempfile::tempdir().expect("vault");
            let invalid_root = invalid_vault.path();
            fs::write(invalid_root.join("dep-invalid.md"), b"id::\xff")
                .expect("write dep invalid");
            fs::write(invalid_root.join("rel-invalid.md"), b"LINKS\xff")
                .expect("write rel invalid");
            write_note(invalid_root, "plain.md", "plain text\n");
            let invalid_ctx = context(invalid_root, &["starts after"], true);
            let dep_snapshot = invalid_ctx.dependents_snapshot();
            let rel_snapshot = invalid_ctx.relation_sources_snapshot();
            // Each snapshot is incomplete on its own invalid note.
            assert!(!dep_snapshot.snapshot_complete(&BTreeMap::new()));
            assert!(!rel_snapshot.snapshot_complete(&BTreeMap::new()));
            // Masking only the relation-invalid note restores the relation
            // snapshot even though the dependency-invalid note is still
            // uncovered, proving the dependency-only bytes never tainted it.
            assert!(rel_snapshot.snapshot_complete(&BTreeMap::from([(
                PathBuf::from("rel-invalid.md"),
                None,
            )])));
            // And the reverse: masking only the dependency-invalid note
            // restores the dependency snapshot.
            assert!(dep_snapshot.snapshot_complete(&BTreeMap::from([(
                PathBuf::from("dep-invalid.md"),
                Some("- [ ] #task Replaced ^replacement\n".to_string()),
            )])));
            // Marker-negative bytes are never retained.
            assert!(dep_snapshot.get(Path::new("plain.md")).is_none());
            assert!(rel_snapshot.get(Path::new("plain.md")).is_none());
        }
    }

    #[test]
    fn matched_source_dependency_proof_skips_unrelated_id_notes() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "source.md",
            concat!(
                "- [ ] #task Source [dependsOn:: dep] ^s\n",
                "  - 🪢 **LINKS:** starts after [[target#^t]]\n",
            ),
        );
        write_note(
            root,
            "dep.md",
            "- [ ] #task Open prerequisite [id:: dep] ^d\n",
        );
        write_note(
            root,
            "target.md",
            "- [x] #task Done target [id:: target] ^t\n",
        );
        write_note(
            root,
            "unrelated.md",
            "- [ ] #task Unrelated [id:: unrelated] ^u\n",
        );
        write_note(
            root,
            "other.md",
            "- [ ] #task Other waiter [id:: other] ^o\n",
        );
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let result =
            collect(&ctx, &mut planner, root, &[event("target.md", "t")]);
        let paths: BTreeSet<_> = result
            .dependency_notes
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(
            paths,
            BTreeSet::from([
                PathBuf::from("dep.md"),
                PathBuf::from("source.md"),
            ])
        );
        assert!(!paths.contains(Path::new("unrelated.md")));
        assert!(!paths.contains(Path::new("other.md")));
        assert!(!paths.contains(Path::new("target.md")));
    }

    #[test]
    fn relation_snapshot_drops_links_notes_without_event_token() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "source.md",
            "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
        );
        write_note(
            root,
            "other.md",
            "- [ ] #task Other ^o\n  - 🪢 **LINKS:** starts after [[other#^x]]\n",
        );
        write_note(root, "target.md", "- [x] #task Target ^t\n");
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let _ = collect(&ctx, &mut planner, root, &[event("target.md", "t")]);
        let relation = ctx.relation_sources_snapshot();
        assert!(relation.get(Path::new("source.md")).is_some());
        assert!(
            relation.get(Path::new("other.md")).is_none(),
            "LINKS notes that cannot match this close's ^id stay out of the relation snapshot"
        );
        assert!(
            relation.snapshot_complete(&BTreeMap::new()),
            "dropping a readable non-matching LINKS note must not taint the snapshot"
        );
    }

    #[test]
    fn unreadable_links_note_taints_when_event_token_does_not_match() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(root, "target.md", "- [x] #task Target ^t\n");
        fs::write(
            root.join("poison.md"),
            [b"LINKS".as_slice(), &[0xff]].concat(),
        )
        .expect("poison");
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let result =
            collect(&ctx, &mut planner, root, &[event("target.md", "t")]);
        assert!(
            !result.input.relation_snapshot_complete,
            "an unreadable LINKS note taints even without this close's ^id"
        );
        assert!(ctx
            .relation_sources_snapshot()
            .get(Path::new("poison.md"))
            .is_none());
    }

    #[test]
    fn zero_matching_sources_returns_empty_source_and_dependency_notes() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "source.md",
            "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[other#^o]]\n",
        );
        write_note(root, "target.md", "- [x] #task Target ^t\n");
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let result =
            collect(&ctx, &mut planner, root, &[event("target.md", "t")]);
        assert!(result.source_notes.is_empty());
        assert!(result.dependency_notes.is_empty());
    }

    #[test]
    fn marker_negative_target_loads_through_planner_and_missing_is_unavailable()
    {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_note(
            root,
            "source.md",
            "- [ ] #task Source ^s\n  - 🪢 **LINKS:** starts after [[target#^t]]\n",
        );
        // Neither dependency nor relation markers: must still reach R6 via
        // one narrow planner read.
        write_note(root, "target.md", "- [ ] #task Target ^t\n");
        let ctx = context(root, &["starts after"], true);
        let mut planner = CaptureBatchPlanner::default();
        let index =
            super::super::dependencies::staged_note_index(&ctx, &planner, root);
        let result = collect_relation_sources(
            &ctx,
            &mut planner,
            root,
            &[event("target.md", "t")],
            &[],
            &[],
            &settings(root),
            &index,
        )
        .expect("collect")
        .expect("candidate passes gate");
        assert_eq!(result.source_notes.len(), 1);
        // The marker-negative target is carried as a target note, not as
        // unavailable, and the planner holds it after exactly one narrow
        // read (the shared pass already read it once and dropped it).
        assert!(result
            .target_notes
            .iter()
            .any(|(path, _)| path == Path::new("target.md")));
        assert!(!result
            .input
            .unavailable_targets
            .contains(Path::new("target.md")));
        assert!(planner.by_path.contains_key(&root.join("target.md")));
        assert_eq!(
            ctx.snapshot_read_counts().get(Path::new("target.md")),
            Some(&1)
        );

        // A missing target still lands in unavailable_targets.
        let missing_index =
            super::super::dependencies::staged_note_index(&ctx, &planner, root);
        let missing = collect_relation_sources(
            &ctx,
            &mut planner,
            root,
            &[event("missing.md", "m")],
            &[],
            &[],
            &settings(root),
            &missing_index,
        )
        .expect("collect missing")
        .expect("candidate passes gate");
        assert!(missing.source_notes.is_empty());
        assert!(missing.dependency_notes.is_empty());
        assert!(missing
            .input
            .unavailable_targets
            .contains(Path::new("missing.md")));
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
