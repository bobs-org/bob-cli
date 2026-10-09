//! Guarded v2 reading-task insertion through capture's write path.
//!
//! `insert_ref_task(bob_dir, destination, line, children)` rereads the
//! destination (and its `done_tasks` archive) at execution, allocates the
//! final `^ref-<slug>` against fresh bytes, combines the reading line with
//! optional pre-formatted child bullets, and writes through capture's
//! `insert_task_line` + `write_staged_files` (Tasks-section placement,
//! no-section behavior, child placement, and line endings preserved).
//! Bounded retries cover only true preimage mismatches; permission/IO
//! errors propagate.
//!
//! Concrete types (for `migrate-tasks` reuse):
//! - `bob_dir: &Path` — vault root.
//! - `destination: &Path` — absolute destination note path.
//! - `line: &str` — rendered reading line with a preview `^id` (replaced
//!   with the final allocated ID at execution).
//! - `children: &[String]` — fully formatted child bullet lines
//!   (e.g. `"  - ⚠️ …"`), joined with `\n` under the task line.
//! - returns `InsertedRefTask { block_id, placement, task_line }` where
//!   `placement` is `capture::Placement` and `task_line` is the final line
//!   (with final ID). Render the embed with
//!   `line::managed_embed_line(residence, &result.block_id)`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::native::capture::{insert_task_line, Placement, StagedTextFile};
use crate::native::collect_done::{
    block_ids_in_markdown, trailing_block_id_in_line,
};

use super::line::allocate_ref_block_id;

/// A completed guarded insertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InsertedRefTask {
    pub block_id: String,
    pub placement: Placement,
    pub task_line: String,
}

/// Insert one reading task into `destination` through the shared capture
/// write path. See the module docs for argument and result types.
pub(crate) fn insert_ref_task(
    bob_dir: &Path,
    destination: &Path,
    line: &str,
    children: &[String],
) -> Result<InsertedRefTask, String> {
    let stem = destination
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("reading");
    // Preview stem for the line's ref target: the trailing ^id's slug hint
    // is the ref-note stem, but allocation must be unique against the
    // destination, so derive the base from the line's own preview ID when
    // present and fall back to the destination stem.
    let preview_stem_owned = ref_stem_hint_from_line(line);
    let preview_stem = preview_stem_owned.as_deref().unwrap_or(stem);
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        let dest_contents = match fs::read_to_string(destination) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Missing destination (e.g. a never-created inbox): create
                // it with the task block, mirroring capture's
                // missing-target behavior.
                String::new()
            }
            Err(e) => {
                return Err(format!("read {}: {e}", destination.display()));
            }
        };
        // Distinguish real read errors from a missing file: a missing file
        // yields empty contents above; other errors already returned.
        let existed = destination.exists();
        let archive_contents =
            archive_contents_for(bob_dir, destination, &dest_contents);
        let mut taken: BTreeSet<String> =
            block_ids_in_markdown(&dest_contents).into_iter().collect();
        if let Some(archive) = archive_contents.as_deref() {
            taken.extend(block_ids_in_markdown(archive));
        }
        let block_id =
            allocate_ref_block_id(preview_stem, &|id| taken.contains(id));
        let task_line = with_final_block_id(line, &block_id);
        let full_block = if children.is_empty() {
            task_line.clone()
        } else {
            format!("{task_line}\n{}", children.join("\n"))
        };
        let (updated, placement) = if existed {
            insert_task_line(&dest_contents, &full_block)
        } else {
            // New file: mirror capture's missing-target write.
            crate::native::capture::validate_target_parent(destination)
                .map_err(|e| e.message.clone())?;
            (format!("{full_block}\n"), Placement::Created)
        };
        if updated == dest_contents && existed {
            return Ok(InsertedRefTask {
                block_id,
                placement: Placement::Unchanged,
                task_line,
            });
        }
        let staged = StagedTextFile {
            target: destination.to_path_buf(),
            target_existed: existed,
            original_target: dest_contents,
            updated_target: updated,
        };
        match crate::native::capture::write_staged_files(std::slice::from_ref(
            &staged,
        )) {
            Ok(()) => {
                return Ok(InsertedRefTask {
                    block_id,
                    placement,
                    task_line,
                });
            }
            Err(error) => {
                let msg = error.message.clone();
                if is_preimage_mismatch(&msg) && attempt < 3 {
                    continue;
                }
                return Err(msg);
            }
        }
    }
}

fn is_preimage_mismatch(msg: &str) -> bool {
    msg.contains("changed on disk after planning")
        || msg.contains("created on disk after planning")
        || msg.contains("deleted on disk after planning")
}

fn with_final_block_id(line: &str, block_id: &str) -> String {
    if let Some(old) = trailing_block_id_in_line(line) {
        let pos = line.rfind(&old).unwrap_or(line.len());
        // Replace only the trailing occurrence.
        let start = line[..pos].len() + "^".len();
        let _ = start;
        // Find the caret before the old id at the trailing position.
        if let Some(caret) = line.rfind(format!("^{old}").as_str()) {
            return format!("{}{}", &line[..caret + 1], block_id);
        }
        return line.to_string();
    }
    format!("{line} ^{block_id}")
}

/// Best-effort ref-note stem hint from the line's first wikilink target
/// (`ref/<type>/<stem>` → `<stem>`); used only to seed ID allocation.
/// Uniqueness never depends on the hint — the fresh destination, archive,
/// and retry loop enforce it.
fn ref_stem_hint_from_line(line: &str) -> Option<String> {
    let open = line.find("[[")?;
    let close = line[open..].find("]]")?;
    let inside = &line[open + 2..open + close];
    let target = inside.split('|').next()?.split('#').next()?.trim();
    let stem = target.rsplit('/').next()?.trim();
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_string())
}

fn archive_contents_for(
    bob_dir: &Path,
    destination: &Path,
    dest_contents: &str,
) -> Option<String> {
    let archive_rel = archive_rel_for_destination(destination, dest_contents)?;
    let archive_path = bob_dir.join(&archive_rel);
    fs::read_to_string(&archive_path).ok()
}

fn archive_rel_for_destination(
    destination: &Path,
    dest_contents: &str,
) -> Option<String> {
    // Prefer the note's own `done_tasks: "[[done/...]]"` link.
    for line in dest_contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("done_tasks:") {
            if let Some(start) = trimmed.find("[[") {
                if let Some(end) = trimmed[start..].find("]]") {
                    let target = &trimmed[start + 2..start + end];
                    let mut rel = target.trim().to_string();
                    if !rel.ends_with(".md") {
                        rel.push_str(".md");
                    }
                    return Some(rel);
                }
            }
        }
    }
    // Default `done/<stem>_done.md` for root notes.
    if destination.components().count() <= 2 {
        if let Some(stem) = destination.file_stem().and_then(|s| s.to_str()) {
            return Some(format!("done/{stem}_done.md"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_destination_is_created() {
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("mac_inbox.md");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let out = insert_ref_task(bob, &dest, line, &[]).expect("insert");
        assert_eq!(out.block_id, "ref-x");
        assert_eq!(out.placement, Placement::Created);
        let contents = fs::read_to_string(&dest).expect("read");
        assert!(contents.contains("^ref-x"), "{contents}");
    }

    #[test]
    fn collision_reallocates_and_children_join() {
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("sase.md");
        fs::write(
            &dest,
            "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n## Tasks\n\n- [ ] #task #ref [[ref/papers/a|A]] [created::2026-10-09] ^ref-x\n",
        )
        .expect("write");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let out = insert_ref_task(
            bob,
            &dest,
            line,
            &["  - ⚠️ parent 'nope' is not an open area or project · refile me with Ctrl+Shift+M".to_string()],
        )
        .expect("insert");
        assert_eq!(out.block_id, "ref-x-2");
        let contents = fs::read_to_string(&dest).expect("read");
        assert!(contents.contains("^ref-x-2"), "{contents}");
        assert!(contents.contains("⚠️"), "{contents}");
    }
}
