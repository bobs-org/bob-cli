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

pub(super) fn staged_text(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn row(
        line: usize,
        text: &str,
        block_id: &str,
        settled_ordinal: Option<usize>,
        key: &str,
    ) -> TaskCompleteUnblockedJson {
        TaskCompleteUnblockedJson {
            note_path: "deps.md".to_string(),
            block_id: block_id.to_string(),
            line,
            text: text.to_string(),
            previous_status_symbol: '?',
            previous_status_name: "Blocked".to_string(),
            status_symbol: ' ',
            status_name: "Ready".to_string(),
            inbox: false,
            unblocked_by: Vec::new(),
            link: None,
            not_linked: Some("not_selected".to_string()),
            trigger: "recovery",
            hide_removed: false,
            offer: Some(TaskCompleteOfferJson {
                key: key.to_string(),
                n: 1,
                group: "hand_off",
                selected: false,
                verdict: None,
                verdict_message: None,
                verdict_ticklers: Vec::new(),
                block_id_needed: false,
                project: None,
                destination: TaskCompleteOfferDestinationJson {
                    day_file: "today.md".to_string(),
                    entry_name: "work".to_string(),
                    entry_role: "slot",
                    fallback: false,
                },
                after: None,
                moved_from: None,
                merged_from: None,
                note: None,
                close: None,
                order_key: (0, (0, 0), 0, PathBuf::from("deps.md"), 0),
                settled_ordinal,
            }),
            relation: None,
        }
    }

    fn line_after(
        row: &TaskCompleteUnblockedJson,
        text: &str,
    ) -> Option<usize> {
        let vault = tempfile::tempdir().expect("vault");
        successor_task_line_after_write(row, text, vault.path())
    }

    #[test]
    fn idless_twin_above_does_not_steal_the_landing() {
        let text = concat!(
            "- [ ] #task Beta\n",
            "- [ ] #task Beta [dependsOn:: bar]\n",
        );
        let found = line_after(&row(2, "Beta", "", None, "deps.md@beta"), text);
        assert_eq!(found, Some(2));
    }

    #[test]
    fn settled_ordinal_skips_a_twin_that_minted_an_id() {
        let text = concat!(
            "- [ ] #task Beta ^minted\n",
            "- [ ] #task Beta [dependsOn:: bar]\n",
            "- [ ] #task Beta [dependsOn:: bar]\n",
        );
        let found =
            line_after(&row(2, "Beta", "", Some(1), "deps.md@beta~2"), text);
        assert_eq!(found, Some(2));
    }

    #[test]
    fn duplicate_block_id_fails_closed() {
        let text =
            concat!("- [ ] #task Alpha ^dup\n", "- [ ] #task Beta ^dup\n",);
        let found =
            line_after(&row(2, "Beta", "dup", None, "deps.md#^dup"), text);
        assert_eq!(found, None);
    }

    #[test]
    fn js14_unsettled_identity_fails_closed() {
        let text = "- [ ] #task Beta [dependsOn:: bar]\n";
        let out_of_range =
            line_after(&row(1, "Beta", "", Some(3), "deps.md@beta"), text);
        assert_eq!(out_of_range, None);
        let line_mismatch =
            line_after(&row(9, "Beta", "", None, "deps.md@beta"), text);
        assert_eq!(line_mismatch, None);
    }
}
