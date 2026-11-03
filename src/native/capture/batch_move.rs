//! Shared capture adapter for snapshot-planned task moves.
//!
//! This binds move-engine postimages to the batch's staged overlay, stages
//! every destination before its source in the eventual replacement order, and
//! stamps moved roots through the same `stamp_fresh` rule used by inbox routes.

use super::*;
use crate::native::freshness::placement::stamp_fresh;
use crate::native::task_move::{
    plan_task_move, MovePlan, MoveRecord, MoveRequest, PlanError, VaultSnapshot,
};

#[derive(Debug)]
pub(super) enum BatchMoveError {
    Plan(PlanError),
    Capture(CaptureError),
}

#[allow(dead_code)] // Used by the project-note execution phase.
pub(super) fn apply_batch_move(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    today: NaiveDate,
    request: &MoveRequest,
) -> Result<MovePlan, BatchMoveError> {
    let snapshot = destination_snapshot(bob_dir, planner)
        .map_err(BatchMoveError::Capture)?;
    apply_batch_move_from_snapshot(
        planner, bob_dir, settings, today, request, &snapshot,
    )
}

pub(super) fn apply_batch_move_from_snapshot(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    today: NaiveDate,
    request: &MoveRequest,
    snapshot: &VaultSnapshot,
) -> Result<MovePlan, BatchMoveError> {
    let plan =
        plan_task_move(snapshot, request).map_err(BatchMoveError::Plan)?;
    let (source, destination) = plan
        .moves
        .first()
        .map(|movement| {
            (
                Some(movement.source_path.as_path()),
                Some(movement.dest_path.as_path()),
            )
        })
        .unwrap_or((None, None));
    stage_move_postimages(
        planner,
        bob_dir,
        snapshot,
        &plan.postimages,
        source,
        destination,
    )
    .map_err(BatchMoveError::Capture)?;
    stamp_move_roots(planner, bob_dir, settings, today, &plan.moves)
        .map_err(BatchMoveError::Capture)?;
    Ok(plan)
}

pub(super) fn destination_snapshot(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
) -> Result<VaultSnapshot, CaptureError> {
    let paths = collect_done::link_repair_markdown_files(bob_dir)
        .map_err(|error| CaptureError::io(error.to_string()))?;
    let mut notes = BTreeMap::new();
    for path in paths {
        let relative = path.strip_prefix(bob_dir).map_err(|error| {
            CaptureError::io(format!(
                "note {} is outside vault {}: {error}",
                path.display(),
                bob_dir.display()
            ))
        })?;
        let contents = fs::read_to_string(&path).map_err(|error| {
            CaptureError::io(format!(
                "failed to read {}: {error}",
                path.display()
            ))
        })?;
        notes.insert(relative.to_path_buf(), contents);
    }
    for file in &planner.files {
        let Ok(relative) = file.path.strip_prefix(bob_dir) else {
            continue;
        };
        if file.present {
            notes.insert(relative.to_path_buf(), file.current.clone());
        } else {
            notes.remove(relative);
        }
    }
    Ok(VaultSnapshot::from_notes(notes))
}

pub(super) fn stage_move_postimages(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    snapshot: &VaultSnapshot,
    postimages: &BTreeMap<PathBuf, String>,
    source: Option<&Path>,
    destination: Option<&Path>,
) -> Result<(), CaptureError> {
    for (path, postimage) in postimages {
        let role = if Some(path.as_path()) == destination {
            MoveWriteRole::Destination
        } else if Some(path.as_path()) == source {
            MoveWriteRole::Source
        } else {
            MoveWriteRole::Other
        };
        planner.stage_snapshot_postimage(
            &bob_dir.join(path),
            snapshot.notes.get(path).cloned(),
            postimage.clone(),
            role,
        )?;
    }
    Ok(())
}

fn stamp_move_roots(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
    today: NaiveDate,
    moves: &[MoveRecord],
) -> Result<(), CaptureError> {
    for movement in moves {
        let destination = bob_dir.join(&movement.dest_path);
        let dest_text = planner.peek_text(&destination).ok_or_else(|| {
            CaptureError::io(format!(
                "capture move lost destination note {}",
                movement.dest_path.display()
            ))
        })?;
        let root = movement.dest_line.saturating_sub(1);
        let root_line = dest_text.lines().nth(root).ok_or_else(|| {
            CaptureError::io(format!(
                "capture move lost landing line {} in {}",
                movement.dest_line,
                movement.dest_path.display()
            ))
        })?;
        if let Some(id) =
            movement.new_block_id.as_deref().filter(|id| !id.is_empty())
        {
            let scan = note_tasks::scan(&dest_text, settings);
            let holds = match scan.by_block_id(id) {
                note_tasks::BlockIdLookup::Found(task) => {
                    task.line_index == root
                }
                _ => false,
            };
            if !holds {
                return Err(CaptureError::io(format!(
                    "capture move lost block ^{id} at line {} in {}",
                    movement.dest_line,
                    movement.dest_path.display()
                )));
            }
        }
        let stamped = stamp_fresh(root_line, today);
        if stamped.line != root_line {
            let mut rebuilt = dest_text
                .lines()
                .enumerate()
                .map(|(number, line)| {
                    if number == root {
                        stamped.line.clone()
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            if dest_text.ends_with('\n') {
                rebuilt.push('\n');
            }
            planner.stage(&destination, rebuilt)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_replacement_order_writes_destinations_before_sources() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let source = vault.join("source.md");
        let repair = vault.join("repair.md");
        let destination = vault.join("destination.md");
        for (path, text) in [
            (&source, "source before\n"),
            (&repair, "repair before\n"),
            (&destination, "destination before\n"),
        ] {
            fs::write(path, text).expect("write note");
        }

        let mut planner = CaptureBatchPlanner::default();
        for path in [&source, &repair, &destination] {
            planner.ensure_loaded(path).expect("load note");
            planner
                .stage(
                    path,
                    format!(
                        "{} after\n",
                        path.file_stem().unwrap().to_string_lossy()
                    ),
                )
                .expect("stage note");
        }
        let snapshot = VaultSnapshot::from_notes([
            (PathBuf::from("source.md"), "source before\n".to_string()),
            (PathBuf::from("repair.md"), "repair before\n".to_string()),
            (
                PathBuf::from("destination.md"),
                "destination before\n".to_string(),
            ),
        ]);
        let postimages = BTreeMap::from([
            (PathBuf::from("source.md"), "source moved\n".to_string()),
            (PathBuf::from("repair.md"), "repair links\n".to_string()),
            (
                PathBuf::from("destination.md"),
                "destination includes task\n".to_string(),
            ),
        ]);
        stage_move_postimages(
            &mut planner,
            vault,
            &snapshot,
            &postimages,
            Some(Path::new("source.md")),
            Some(Path::new("destination.md")),
        )
        .expect("stage move postimages");

        let targets = planner
            .into_staged_files()
            .into_iter()
            .map(|file| {
                file.target
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(targets, ["destination.md", "repair.md", "source.md"]);
    }
}
