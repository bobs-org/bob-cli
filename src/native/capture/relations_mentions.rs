//! Capture mention-mirror courtesy writes (`docs/task-relations.md` §19).
//!
//! After every item is planned, and before commit, this pass diffs the
//! mention pairs of each note the capture writes and stages the
//! targets' LINKED FROM entries (and source mints) in the same
//! all-or-nothing batch. Reconcile stays the authority: a skipped
//! mirror never fails the capture.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::native::{
    task_dependencies::{
        self, block_link_spans, parse as dependency_parse, DependencyLine,
    },
    task_relations::{
        mentions::{has_unclosed_fence, scan_mentions, MentionPair},
        parse as relation_parse, MENTION_LABEL,
    },
    task_status_hooks::{self, FileScan},
    vault_links::{target_to_markdown_path, NoteIndex},
};

struct WrittenNote {
    absolute: PathBuf,
    relative: PathBuf,
    original: String,
    current: String,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PairKey {
    source_path: PathBuf,
    source: String,
    target_path: PathBuf,
    target_id: String,
}

struct TrackedPair {
    pair: MentionPair,
}

/// Diff mention pairs on notes this capture writes and stage T's
/// LINKED FROM add/remove (plus S's mint) through the existing
/// mirror helpers.
pub(super) fn plan_mention_mirrors(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    items: &mut [PlannedCaptureItem],
    warnings: &mut Vec<CaptureWarning>,
) -> Result<(), CaptureError> {
    if items.is_empty() {
        return Ok(());
    }
    let written = written_notes(planner, bob_dir);
    if written.is_empty()
        || written.iter().all(|note| {
            !note.original.contains("#^") && !note.current.contains("#^")
        })
    {
        return Ok(());
    }
    if !ctx.linked_from_mentions_enabled() {
        return Ok(());
    }

    let index = super::dependencies::staged_note_index(ctx, planner, bob_dir);
    let extra_targets =
        collect_link_targets(&written, &index, bob_dir, planner);
    let index = index.with_additional(extra_targets.iter().cloned());
    let settings = ctx.settings().clone();
    let pre_files =
        scan_files(&written, &extra_targets, planner, bob_dir, &settings, true);
    let post_files = scan_files(
        &written,
        &extra_targets,
        planner,
        bob_dir,
        &settings,
        false,
    );
    let written_relatives: BTreeSet<PathBuf> =
        written.iter().map(|note| note.relative.clone()).collect();
    let pre = tracked_pairs(
        &pre_files,
        &index,
        &written_relatives,
        Some(&post_files),
    );
    let post = tracked_pairs(&post_files, &index, &written_relatives, None);

    let pre_keys: BTreeSet<PairKey> = pre.keys().cloned().collect();
    let post_keys: BTreeSet<PairKey> = post.keys().cloned().collect();

    let mut added: Vec<&TrackedPair> = post_keys
        .difference(&pre_keys)
        .filter_map(|key| post.get(key))
        .collect();
    added.sort_by(|left, right| {
        left.pair
            .source_path
            .cmp(&right.pair.source_path)
            .then(left.pair.source_line.cmp(&right.pair.source_line))
            .then(left.pair.target_path.cmp(&right.pair.target_path))
            .then(left.pair.target_block_id.cmp(&right.pair.target_block_id))
    });
    let mut removed: Vec<&TrackedPair> = pre_keys
        .difference(&post_keys)
        .filter_map(|key| pre.get(key))
        .collect();
    removed.sort_by(|left, right| {
        left.pair
            .source_path
            .cmp(&right.pair.source_path)
            .then(left.pair.source_line.cmp(&right.pair.source_line))
            .then(left.pair.target_path.cmp(&right.pair.target_path))
            .then(left.pair.target_block_id.cmp(&right.pair.target_block_id))
    });

    let mut minted_sources: BTreeMap<(PathBuf, usize), String> =
        BTreeMap::new();
    let mut prepared = Vec::new();
    // Mint every eligible source before inserting any mirror children. Mints
    // preserve line numbers; a same-note mirror can shift all later sources.
    for tracked in added {
        if tracked.pair.source_uncertain {
            continue;
        }
        if let Some((item_index, source_id)) = prepare_added_pair(
            ctx,
            planner,
            bob_dir,
            &index,
            items,
            tracked,
            &post_files,
            &mut minted_sources,
        )? {
            prepared.push((tracked, item_index, source_id));
        }
    }
    for (tracked, item_index, source_id) in prepared {
        let pair = &tracked.pair;
        if let Some(update) = stage_linked_from_add(
            ctx,
            planner,
            bob_dir,
            &index,
            &pair.target_path,
            &pair.target_block_id,
            &(pair.source_path.clone(), source_id),
            Some(MENTION_LABEL),
            warnings,
        )? {
            items[item_index]
                .result
                .linked_from_line_updates
                .push(update);
        }
    }
    for tracked in removed {
        if tracked.pair.source_uncertain
            || removal_is_uncertain(&tracked.pair, &post_files, &index)
        {
            continue;
        }
        apply_removed_pair(
            ctx, planner, bob_dir, &index, items, warnings, tracked,
        )?;
    }
    Ok(())
}

// A pair disappearing from the scanner is not proof that its link was
// deleted: the postimage may have become unparseable. Check the postimage
// even when it no longer contains a candidate occurrence.
fn removal_is_uncertain(
    pair: &MentionPair,
    files: &[FileScan],
    index: &NoteIndex,
) -> bool {
    let Some(source) = files
        .iter()
        .find(|file| file.relative_path == pair.source_path)
    else {
        return true;
    };
    let lines: Vec<&str> = source.contents.lines().collect();
    if has_unclosed_fence(&lines) {
        return true;
    }
    if let Some(id) = &pair.source_block_id {
        if collect_done::block_ids_in_markdown(&source.contents)
            .iter()
            .filter(|candidate| *candidate == id)
            .count()
            > 1
        {
            return true;
        }
        if let Some(task) = source
            .tasks
            .iter()
            .find(|task| task.block_id.as_ref() == Some(id))
        {
            let fenced =
                task_status_hooks::fenced_lines(&lines, 0..lines.len());
            if relation_parse::links_children_of(
                &lines,
                &fenced,
                task.line_index,
            )
            .iter()
            .any(|child| {
                matches!(
                    child.parsed,
                    crate::native::task_relations::LinksLine::Malformed
                )
            }) || matches!(
                depends_on_verdict(
                    &source.contents,
                    task.line_index,
                    index,
                    &pair.source_path,
                    &pair.target_path,
                    &pair.target_block_id
                ),
                DependsVerdict::Malformed
            ) {
                return true;
            }
        }
    }
    let Some(target) = files
        .iter()
        .find(|file| file.relative_path == pair.target_path)
    else {
        return true;
    };
    matches!(
        depends_on_verdict(
            &target.contents,
            task_line_for_id(&target.contents, &pair.target_block_id)
                .unwrap_or(usize::MAX),
            index,
            &pair.target_path,
            &pair.source_path,
            pair.source_block_id.as_deref().unwrap_or_default()
        ),
        DependsVerdict::Malformed
    )
}

fn written_notes(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
) -> Vec<WrittenNote> {
    planner
        .files
        .iter()
        .filter(|file| {
            file.present && (!file.existed || file.current != file.original)
        })
        .filter_map(|file| {
            let relative = file.path.strip_prefix(bob_dir).ok()?.to_path_buf();
            Some(WrittenNote {
                absolute: file.path.clone(),
                relative,
                original: file.original.clone(),
                current: file.current.clone(),
            })
        })
        .collect()
}

fn collect_link_targets(
    written: &[WrittenNote],
    index: &NoteIndex,
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
) -> BTreeSet<PathBuf> {
    let mut targets = BTreeSet::new();
    for note in written {
        for contents in [&note.original, &note.current] {
            collect_targets_from(
                contents,
                &note.relative,
                index,
                bob_dir,
                planner,
                &mut targets,
            );
        }
    }
    targets
}

fn collect_targets_from(
    contents: &str,
    source_relative: &Path,
    index: &NoteIndex,
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
    targets: &mut BTreeSet<PathBuf>,
) {
    for line in contents.lines() {
        for span in block_link_spans(line) {
            if let Some(path) =
                index.resolve(Some(source_relative), &span.target)
            {
                targets.insert(path);
                continue;
            }
            let Some(candidate) = target_to_markdown_path(&span.target) else {
                continue;
            };
            let absolute = bob_dir.join(&candidate);
            if planner.peek_text(&absolute).is_some() || absolute.is_file() {
                targets.insert(candidate);
            }
        }
    }
}

fn scan_files(
    written: &[WrittenNote],
    extra_targets: &BTreeSet<PathBuf>,
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    original: bool,
) -> Vec<FileScan> {
    let mut by_relative: BTreeMap<PathBuf, FileScan> = BTreeMap::new();
    for note in written {
        let contents = if original {
            note.original.as_str()
        } else {
            note.current.as_str()
        };
        by_relative.insert(
            note.relative.clone(),
            file_scan(&note.absolute, &note.relative, contents, settings),
        );
    }
    for relative in extra_targets {
        if by_relative.contains_key(relative) {
            continue;
        }
        let absolute = bob_dir.join(relative);
        let Some(contents) = note_snapshot(planner, &absolute, original) else {
            continue;
        };
        by_relative.insert(
            relative.clone(),
            file_scan(&absolute, relative, &contents, settings),
        );
    }
    by_relative.into_values().collect()
}

fn note_snapshot(
    planner: &CaptureBatchPlanner,
    absolute: &Path,
    original: bool,
) -> Option<String> {
    if let Some(index) = planner.by_path.get(absolute) {
        let file = &planner.files[*index];
        if !file.present && original {
            return None;
        }
        return Some(if original {
            file.original.clone()
        } else {
            file.current.clone()
        });
    }
    fs::read_to_string(absolute).ok()
}

fn file_scan(
    absolute: &Path,
    relative: &Path,
    contents: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> FileScan {
    FileScan {
        path: absolute.to_path_buf(),
        relative_path: relative.to_path_buf(),
        contents: contents.to_string(),
        tasks: task_status_hooks::parse_tasks(contents, settings),
    }
}

fn tracked_pairs(
    files: &[FileScan],
    index: &NoteIndex,
    written: &BTreeSet<PathBuf>,
    post_files: Option<&[FileScan]>,
) -> BTreeMap<PairKey, TrackedPair> {
    let mut mapped_lines = BTreeMap::new();
    if let Some(post_files) = post_files {
        for file in files {
            if let Some(post) = post_files
                .iter()
                .find(|post| post.relative_path == file.relative_path)
            {
                let before: Vec<&str> = file.contents.lines().collect();
                let after: Vec<&str> = post.contents.lines().collect();
                mapped_lines.insert(
                    file.relative_path.clone(),
                    line_maps(&before, &after).0,
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    for pair in scan_mentions(files, index).pairs {
        if !written.contains(&pair.source_path) {
            continue;
        }
        // Titles are not identities: two id-less tasks with identical text
        // must remain two sources. Map unchanged old rows into the postimage
        // so an insertion elsewhere does not turn an existing pair into new work.
        let source = match &pair.source_block_id {
            Some(id) => format!("#^{id}"),
            None if post_files.is_none() => {
                format!("line:{}", pair.source_line)
            }
            None => match mapped_lines
                .get(&pair.source_path)
                .and_then(|lines| lines.get(&pair.source_line))
            {
                Some(line) => format!("line:{line}"),
                None => format!("removed:{}", pair.source_line),
            },
        };
        let key = PairKey {
            source_path: pair.source_path.clone(),
            source,
            target_path: pair.target_path.clone(),
            target_id: pair.target_block_id.clone(),
        };
        out.insert(key, TrackedPair { pair });
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn prepare_added_pair(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    items: &mut [PlannedCaptureItem],
    tracked: &TrackedPair,
    post_files: &[FileScan],
    minted_sources: &mut BTreeMap<(PathBuf, usize), String>,
) -> Result<Option<(usize, String)>, CaptureError> {
    let pair = &tracked.pair;
    if m7_blocks_mention(planner, bob_dir, index, post_files, pair)? {
        return Ok(None);
    }
    let source_text = post_files
        .iter()
        .find(|file| file.relative_path == pair.source_path)
        .and_then(|file| file.contents.lines().nth(pair.source_line));
    let item_index = owning_item(items, pair, source_text);
    if let Some(skip) = skip_reason_for_target(ctx, planner, bob_dir, pair)? {
        items[item_index]
            .result
            .mirror_skipped
            .push(MirrorSkippedJson {
                path: display_relative(&pair.target_path),
                block_id: pair.target_block_id.clone(),
                reason: skip.as_str(),
            });
        return Ok(None);
    }

    let source_id = match source_block_id(
        ctx,
        planner,
        bob_dir,
        items,
        item_index,
        pair,
        source_text,
        minted_sources,
    )? {
        Some(id) => id,
        None => return Ok(None),
    };
    Ok(Some((item_index, source_id)))
}

#[allow(clippy::too_many_arguments)]
fn apply_removed_pair(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    items: &mut [PlannedCaptureItem],
    warnings: &mut Vec<CaptureWarning>,
    tracked: &TrackedPair,
) -> Result<(), CaptureError> {
    let pair = &tracked.pair;
    let Some(source_id) = pair.source_block_id.clone() else {
        return Ok(());
    };
    let item_index = owning_item(items, pair, None);
    if let Some(skip) = skip_reason_for_target(ctx, planner, bob_dir, pair)? {
        items[item_index]
            .result
            .mirror_skipped
            .push(MirrorSkippedJson {
                path: display_relative(&pair.target_path),
                block_id: pair.target_block_id.clone(),
                reason: skip.as_str(),
            });
        return Ok(());
    }
    if let Some(update) = stage_linked_from_remove(
        ctx,
        planner,
        bob_dir,
        index,
        &pair.target_path,
        &pair.target_block_id,
        &(pair.source_path.clone(), source_id),
        warnings,
    )? {
        items[item_index]
            .result
            .linked_from_line_updates
            .push(update);
    }
    Ok(())
}

fn skip_reason_for_target(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    pair: &MentionPair,
) -> Result<Option<MirrorSkipReason>, CaptureError> {
    if !ctx.linked_from_lines_enabled() {
        return Ok(Some(MirrorSkipReason::Disabled));
    }
    if task_dependencies::is_archive_path(&pair.target_path) {
        return Ok(Some(MirrorSkipReason::ArchivedTarget));
    }
    let target_absolute = absolute_note(bob_dir, &pair.target_path);
    if ctx.previous_daily() == Some(target_absolute.as_path()) {
        return Ok(Some(MirrorSkipReason::PreviousDailyTarget));
    }
    if linked_from_is_malformed(
        ctx,
        planner,
        bob_dir,
        &pair.target_path,
        &pair.target_block_id,
    )? {
        return Ok(Some(MirrorSkipReason::MalformedLine));
    }
    Ok(None)
}

fn source_block_id(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    items: &mut [PlannedCaptureItem],
    item_index: usize,
    pair: &MentionPair,
    source_text: Option<&str>,
    minted_sources: &mut BTreeMap<(PathBuf, usize), String>,
) -> Result<Option<String>, CaptureError> {
    if let Some(id) = &pair.source_block_id {
        return Ok(Some(id.clone()));
    }
    if task_dependencies::is_archive_path(&pair.source_path)
        || ctx.previous_daily()
            == Some(absolute_note(bob_dir, &pair.source_path).as_path())
    {
        return Ok(None);
    }
    let key = (pair.source_path.clone(), pair.source_line);
    if let Some(id) = minted_sources.get(&key) {
        return Ok(Some(id.clone()));
    }
    let absolute = absolute_note(bob_dir, &pair.source_path);
    let contents = current_note_text(planner, &absolute)?;
    let task_line = contents
        .lines()
        .nth(pair.source_line)
        .unwrap_or_default()
        .to_string();
    if let Some(existing) =
        crate::native::collect_done::trailing_block_id_in_line(&task_line)
    {
        minted_sources.insert(key, existing.clone());
        return Ok(Some(existing));
    }
    let minted = mint_and_append_block_id(
        ctx,
        planner,
        bob_dir,
        &pair.source_path,
        pair.source_line,
        &task_line,
    )?;
    record_mention_mint(items, item_index, pair, source_text, &minted);
    minted_sources.insert(key, minted.clone());
    Ok(Some(minted))
}

fn record_mention_mint(
    items: &mut [PlannedCaptureItem],
    item_index: usize,
    pair: &MentionPair,
    source_text: Option<&str>,
    minted: &str,
) {
    let item = &mut items[item_index].result;
    if item
        .minted_relation_block_ids
        .iter()
        .any(|row| row.block_id == minted)
    {
        return;
    }
    let path = display_relative(&pair.source_path);
    item.minted_relation_block_ids
        .push(MintedRelationBlockIdJson {
            path: path.clone(),
            line: pair.source_line + 1,
            block_id: minted.to_string(),
            role: MintedRelationRole::Source,
            reason: Some("mention"),
        });
    if item.block_id.is_none()
        && item.relative_target == path
        && source_text.is_some_and(|text| text.trim() == item.task_line.trim())
    {
        item.block_id = Some(minted.to_string());
        if !item.task_line.contains(&format!(" ^{minted}")) {
            let trimmed = item.task_line.trim_end();
            item.task_line = format!("{trimmed} ^{minted}");
        }
    }
}

fn owning_item(
    items: &[PlannedCaptureItem],
    pair: &MentionPair,
    source_text: Option<&str>,
) -> usize {
    let display = display_relative(&pair.source_path);
    items
        .iter()
        .position(|item| {
            let item = &item.result;
            item.relative_target == display
                && (pair
                    .source_block_id
                    .as_ref()
                    .is_some_and(|id| item.block_id.as_ref() == Some(id))
                    || source_text.is_some_and(|text| {
                        text.trim() == item.task_line.trim()
                    })
                    || item.minted_relation_block_ids.iter().any(|mint| {
                        mint.path == display
                            && mint.line == pair.source_line + 1
                            && mint.reason == Some("mention")
                    }))
        })
        .or_else(|| {
            items
                .iter()
                .rposition(|item| item.result.relative_target == display)
        })
        .unwrap_or(items.len().saturating_sub(1))
}

fn m7_blocks_mention(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    post_files: &[FileScan],
    pair: &MentionPair,
) -> Result<bool, CaptureError> {
    let source_absolute = absolute_note(bob_dir, &pair.source_path);
    let source_contents = current_note_text(planner, &source_absolute)?;
    if source_has_relation(
        &source_contents,
        pair.source_line,
        index,
        &pair.source_path,
        &pair.target_path,
        &pair.target_block_id,
    ) {
        return Ok(true);
    }
    match depends_on_verdict(
        &source_contents,
        pair.source_line,
        index,
        &pair.source_path,
        &pair.target_path,
        &pair.target_block_id,
    ) {
        DependsVerdict::Hit | DependsVerdict::Malformed => return Ok(true),
        DependsVerdict::Miss => {}
    }
    let target_contents = post_files
        .iter()
        .find(|file| file.relative_path == pair.target_path)
        .map(|file| file.contents.clone())
        .or_else(|| {
            current_note_text(
                planner,
                &absolute_note(bob_dir, &pair.target_path),
            )
            .ok()
        });
    let Some(target_contents) = target_contents else {
        return Ok(false);
    };
    Ok(target_depends_on_source(
        &target_contents,
        &pair.target_block_id,
        index,
        &pair.target_path,
        &pair.source_path,
        pair.source_block_id.as_deref(),
    ))
}

enum DependsVerdict {
    Hit,
    Miss,
    Malformed,
}

fn source_has_relation(
    contents: &str,
    source_line: usize,
    index: &NoteIndex,
    source_relative: &Path,
    target_relative: &Path,
    target_block: &str,
) -> bool {
    let lines: Vec<&str> = contents.lines().collect();
    if source_line >= lines.len() {
        return false;
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children =
        relation_parse::links_children_of(&lines, &fenced, source_line);
    existing_links_entry(
        &children,
        index,
        source_relative,
        target_relative,
        target_block,
    )
    .is_some()
}

fn depends_on_verdict(
    contents: &str,
    task_line: usize,
    index: &NoteIndex,
    owner_relative: &Path,
    target_relative: &Path,
    target_block: &str,
) -> DependsVerdict {
    let lines: Vec<&str> = contents.lines().collect();
    if task_line >= lines.len() {
        return DependsVerdict::Miss;
    }
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let Some(child) =
        dependency_parse::dependency_child_of(&lines, &fenced, task_line)
    else {
        return DependsVerdict::Miss;
    };
    match child.parsed {
        DependencyLine::Malformed => DependsVerdict::Malformed,
        DependencyLine::Accepted { links, .. } => {
            if links.iter().any(|link| {
                let resolved = if link.target.is_empty() {
                    Some(owner_relative.to_path_buf())
                } else {
                    index.resolve(Some(owner_relative), link.target.trim())
                };
                resolved.as_deref() == Some(target_relative)
                    && link.block_id == target_block
            }) {
                DependsVerdict::Hit
            } else {
                DependsVerdict::Miss
            }
        }
        DependencyLine::Empty | DependencyLine::NotALine => {
            DependsVerdict::Miss
        }
    }
}

fn target_depends_on_source(
    contents: &str,
    target_block: &str,
    index: &NoteIndex,
    target_relative: &Path,
    source_relative: &Path,
    source_block: Option<&str>,
) -> bool {
    let Some(source_block) = source_block else {
        return matches!(
            depends_on_verdict(
                contents,
                task_line_for_id(contents, target_block).unwrap_or(usize::MAX),
                index,
                target_relative,
                source_relative,
                "",
            ),
            DependsVerdict::Malformed
        );
    };
    let Some(task_line) = task_line_for_id(contents, target_block) else {
        return false;
    };
    matches!(
        depends_on_verdict(
            contents,
            task_line,
            index,
            target_relative,
            source_relative,
            source_block,
        ),
        DependsVerdict::Hit | DependsVerdict::Malformed
    )
}

fn task_line_for_id(contents: &str, block_id: &str) -> Option<usize> {
    contents.lines().position(|line| {
        crate::native::collect_done::trailing_block_id_in_line(line).as_deref()
            == Some(block_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanished_occurrences_preserve_mirrors_when_postimage_is_uncertain() {
        let settings = task_status_hooks::TasksSettings {
            global_filter: "#task".into(),
            done_statuses: BTreeSet::from(['x']),
            status_types: BTreeMap::from([(
                ' ',
                note_tasks::TaskStatusType::Todo,
            )]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        };
        let source = "- [ ] #task Source ^source\n";
        let target = "- [ ] #task Target ^target\n";
        let pair = MentionPair {
            source_path: "src.md".into(),
            source_line: 0,
            source_block_id: Some("source".into()),
            target_path: "tgt.md".into(),
            target_block_id: "target".into(),
            occurrence_count: 1,
            source_uncertain: false,
        };
        let index = NoteIndex::from_paths([
            pair.source_path.clone(),
            pair.target_path.clone(),
        ]);
        for (after_source, after_target, uncertain) in [
            (source, target, false),
            ("", target, false),
            ("```\n- [ ] #task Source ^source\n", target, true),
            (
                "- [ ] #task Source ^source\n- [ ] #task Duplicate ^source\n",
                target,
                true,
            ),
            (
                "- [ ] #task Source ^source\n  - 🪢 **LINKS:** broken\n",
                target,
                true,
            ),
            (
                "- [ ] #task Source ^source\n  - ⛓️ **DEPENDS ON:** broken\n",
                target,
                true,
            ),
            (
                source,
                "- [ ] #task Target ^target\n  - ⛓️ **DEPENDS ON:** broken\n",
                true,
            ),
        ] {
            let files = [
                file_scan(
                    Path::new("src.md"),
                    Path::new("src.md"),
                    after_source,
                    &settings,
                ),
                file_scan(
                    Path::new("tgt.md"),
                    Path::new("tgt.md"),
                    after_target,
                    &settings,
                ),
            ];
            assert!(scan_mentions(&files, &index).pairs.is_empty());
            assert_eq!(
                removal_is_uncertain(&pair, &files, &index),
                uncertain,
                "{after_source:?} / {after_target:?}"
            );
        }
    }
}
