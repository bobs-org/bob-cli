//! Resolve a pinned successor against the final staged note text.

use super::*;

pub(super) fn build_capture_jump(
    bob_dir: &Path,
    batch: &PlannedCaptureBatch,
    key: Option<&str>,
) -> Option<super::output::CaptureJumpJson> {
    let key = key?;
    let row = batch
        .items
        .iter()
        .flat_map(|item| super::output::all_offer_rows(&item.result))
        .find(|row| row.offer.as_ref().is_some_and(|offer| offer.key == key))?;
    let offer = row.offer.as_ref()?;
    let outcome = offer.verdict.unwrap_or(if offer.selected {
        "pulled_in"
    } else {
        "left"
    });
    let landing = build_landing(bob_dir, batch, row);
    Some(super::output::CaptureJumpJson {
        key: offer.key.clone(),
        n: offer.n,
        label: row.text.clone(),
        outcome,
        landing,
    })
}

fn build_landing(
    bob_dir: &Path,
    batch: &PlannedCaptureBatch,
    row: &TaskCompleteUnblockedJson,
) -> Option<OpenNoteJson> {
    let movement = batch.routed_landings.iter().find(|movement| {
        movement.source_relative == row.note_path
            && if row.block_id.is_empty() {
                movement.old_block_id.is_none() && movement.old_line == row.line
            } else {
                movement.old_block_id.as_deref() == Some(row.block_id.as_str())
            }
    });
    let absolute = movement
        .map(|movement| movement.dest.clone())
        .unwrap_or_else(|| {
            let path = Path::new(&row.note_path);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                bob_dir.join(path)
            }
        });
    let relative = movement
        .map(|movement| movement.dest_relative.clone())
        .unwrap_or_else(|| {
            super::dependencies::display_relative(Path::new(&row.note_path))
        });
    let block_id = movement
        .and_then(|movement| movement.new_block_id.clone())
        .filter(|id| !id.is_empty())
        .or_else(|| (!row.block_id.is_empty()).then(|| row.block_id.clone()))
        .or_else(|| row.link.as_ref().and_then(minted_link_block_id));
    let text = staged_text(bob_dir, batch, &absolute)?;
    let mut resolved = row.clone();
    resolved.note_path = relative.clone();
    resolved.block_id = block_id.unwrap_or_default();
    let line = super::task_complete::successor_task_line_after_write(
        &resolved, &text, bob_dir,
    )?;
    let label = Path::new(&relative)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&relative)
        .to_string();
    let landing = super::open_notes::landed_note(
        OpenNoteJson {
            path: absolute.display().to_string(),
            relative_path: relative,
            role: "note",
            label,
            line: None,
            r#ref: None,
            block_id: None,
        },
        Some(&text),
        Some(line),
    );
    landing.line.map(|_| landing)
}

fn minted_link_block_id(
    link: &TaskCompleteSuccessorLinkJson,
) -> Option<String> {
    if !link.block_id_created {
        return None;
    }
    let id = link.block_link.rsplit_once('^')?.1.trim_end_matches("]]");
    (!id.is_empty()).then(|| id.to_string())
}

fn staged_text(
    bob_dir: &Path,
    batch: &PlannedCaptureBatch,
    target: &Path,
) -> Option<String> {
    batch
        .text_files
        .iter()
        .find(|file| {
            let staged = if file.target.is_absolute() {
                file.target.clone()
            } else {
                bob_dir.join(&file.target)
            };
            staged == target
        })
        .map(|file| file.updated_target.clone())
        .or_else(|| std::fs::read_to_string(target).ok())
}
