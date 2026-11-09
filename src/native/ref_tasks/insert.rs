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
    block_ids_in_markdown, trailing_block_id_in_line, ArchivePairIndex,
    SourceLookup,
};

use super::line::ReadingIdentity;

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
    insert_ref_task_with_preferred_id(
        bob_dir,
        destination,
        line,
        children,
        None,
        None,
    )
}

/// Insert one reading task, reusing `prefer_block_id` when it is free.
///
/// The executor passes a reopen's old (possibly user-renamed) address here:
/// it is kept verbatim only when absent from the fresh destination bytes
/// and its `done_tasks` archive; otherwise the old ID is suffixed
/// (`<old>-2`, …) under the same collision rules. `None` allocates from the
/// ref stem, or from the confirmed reading (`ref-<root-slug>-vN`, with the
/// `-2` suffix following the version) when one is passed. The archive is
/// never modified, including when an old ID must be suffixed in the live
/// note.
pub(crate) fn insert_ref_task_with_preferred_id(
    bob_dir: &Path,
    destination: &Path,
    line: &str,
    children: &[String],
    prefer_block_id: Option<&str>,
    reading: Option<&ReadingIdentity>,
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
    // The pair index is built once per insertion from one vault scan; the
    // destination entry is rebound to freshly read bytes on every attempt
    // so the binding never goes stale inside the retry loop. Unreadable
    // notes are retained as census failures and refuse only the lookups
    // they could plausibly affect.
    let base_pairs = ArchivePairIndex::read_vault(bob_dir)
        .map_err(|error| format!("refusing to reserve archive IDs: {error}"))?;
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
        // Archive read failures other than not-found propagate: a
        // collision hidden by an unreadable archive must not mint a
        // duplicate ID.
        let pairs = rebind_destination(
            &base_pairs,
            bob_dir,
            destination,
            &dest_contents,
        );
        let archive_contents =
            archive_contents_for(bob_dir, destination, &pairs)?;
        let mut taken: BTreeSet<String> =
            block_ids_in_markdown(&dest_contents).into_iter().collect();
        if let Some(archive) = archive_contents.as_deref() {
            taken.extend(block_ids_in_markdown(archive));
        }
        let is_taken = |id: &str| taken.contains(id);
        let block_id = match prefer_block_id {
            Some(preferred) if !preferred.is_empty() => {
                super::line::allocate_unique_block_id(preferred, &is_taken)
            }
            _ => super::line::allocate_birth_block_id(
                preview_stem,
                reading,
                &is_taken,
            ),
        };
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

/// Rebind the destination entry to freshly read bytes without disk I/O.
///
/// The census may hold an older copy of the destination; the retry loop reads
/// it fresh every attempt, and the binding must follow the fresh bytes.
/// Destinations outside the vault keep the base index (the lookup below then
/// finds no archive).
fn rebind_destination(
    base: &ArchivePairIndex,
    bob_dir: &Path,
    destination: &Path,
    dest_contents: &str,
) -> ArchivePairIndex {
    let Ok(relative) = destination.strip_prefix(bob_dir) else {
        return base.clone();
    };
    base.with_contents(relative, dest_contents)
}

/// The archive bytes whose IDs stay reserved, via the shared pair index.
///
/// A refused binding surfaces as an insertion error rather than silently
/// omitting the reservation and risking a duplicate ID.
fn archive_contents_for(
    bob_dir: &Path,
    destination: &Path,
    pairs: &ArchivePairIndex,
) -> Result<Option<String>, String> {
    let Ok(relative) = destination.strip_prefix(bob_dir) else {
        return Ok(None);
    };
    let bound = match pairs.for_source(relative) {
        SourceLookup::Bound(bound) => bound,
        SourceLookup::Missing { canonical_archive } => {
            return missing_archive_contents(bob_dir, canonical_archive)
        }
        SourceLookup::Refused {
            reason, related, ..
        } => {
            return Err(format!(
                "refusing to reserve archive IDs for {}: {reason} ({})",
                destination.display(),
                related
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    };
    let archive_path = bob_dir.join(&bound.archive);
    match fs::read_to_string(&archive_path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", archive_path.display())),
    }
}

/// Reservation bytes when the census knows no archive for the destination.
///
/// The canonical path usually does not exist (a missing archive reserves
/// nothing). When something is there anyway — a directory in the archive
/// slot, or a file that appeared after the census scan — it is read for the
/// reservation set instead of letting a hidden collision mint a duplicate
/// ID; anything but not-found propagates.
fn missing_archive_contents(
    bob_dir: &Path,
    canonical_archive: Option<std::path::PathBuf>,
) -> Result<Option<String>, String> {
    let Some(relative) = canonical_archive else {
        return Ok(None);
    };
    let archive_path = bob_dir.join(&relative);
    match fs::read_to_string(&archive_path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", archive_path.display())),
    }
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
    fn preferred_id_reused_when_free() {
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("sase.md");
        fs::write(&dest, "# Sase\n").expect("write");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-preview";
        let out = insert_ref_task_with_preferred_id(
            bob,
            &dest,
            line,
            &[],
            Some("ref-my-paper"),
            None,
        )
        .expect("insert");
        assert_eq!(out.block_id, "ref-my-paper");
        let contents = fs::read_to_string(&dest).expect("read");
        assert!(contents.contains("^ref-my-paper"), "{contents}");
        assert!(!contents.contains("^ref-preview"), "{contents}");
    }

    #[test]
    fn preferred_id_suffixed_when_destination_taken() {
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("sase.md");
        fs::write(
            &dest,
            "# Sase\n\n- [ ] #task #ref [[ref/papers/a|A]] [created::2026-10-09] ^ref-my-paper\n",
        )
        .expect("write");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-preview";
        let out = insert_ref_task_with_preferred_id(
            bob,
            &dest,
            line,
            &[],
            Some("ref-my-paper"),
            None,
        )
        .expect("insert");
        // A taken old ID is suffixed in the live note; the archived bytes
        // that forced the suffix stay untouched.
        assert_eq!(out.block_id, "ref-my-paper-2");
    }

    #[test]
    fn archive_collision_blocks_preferred_id() {
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("sase.md");
        fs::write(&dest, "# Sase\n").expect("write");
        fs::create_dir_all(bob.join("done")).expect("done dir");
        fs::write(
            bob.join("done/sase_done.md"),
            "# Done\n\n- [x] #task #ref [[ref/papers/x|X]] [created::2026-10-01] [completion:: 2026-10-02] ^ref-my-paper\n",
        )
        .expect("write archive");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-preview";
        // The default `done/<stem>_done.md` archive must be consulted for
        // absolute vault-root destinations: the preferred ID is taken there.
        let out = insert_ref_task_with_preferred_id(
            bob,
            &dest,
            line,
            &[],
            Some("ref-my-paper"),
            None,
        )
        .expect("insert");
        assert_ne!(out.block_id, "ref-my-paper");
        let archive = fs::read_to_string(bob.join("done/sase_done.md"))
            .expect("read archive");
        assert!(
            !archive.contains(&out.block_id),
            "archive must stay untouched: {archive}"
        );
    }

    #[test]
    fn archive_read_errors_propagate() {
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("sase.md");
        fs::write(&dest, "# Sase\n").expect("write");
        // A directory where the archive file should be: reading it fails
        // with an error other than not-found, which must propagate rather
        // than silently mint a possibly-duplicate ID.
        fs::create_dir_all(bob.join("done/sase_done.md")).expect("dir");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let error = insert_ref_task(bob, &dest, line, &[])
            .expect_err("archive read error must propagate");
        assert!(error.contains("sase_done.md"), "{error}");
    }

    #[test]
    fn nested_archive_without_metadata_stays_reserved() {
        // Nested sources never had a default archive under the old
        // filename rule; the shared lookup binds them by filename.
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("sub/note.md");
        fs::create_dir_all(dest.parent().unwrap()).expect("mkdir");
        fs::write(&dest, "# Note\n").expect("write");
        fs::create_dir_all(bob.join("done/sub")).expect("done dir");
        fs::write(
            bob.join("done/sub/note_done.md"),
            "# Done\n\n- [x] #task #ref [[ref/papers/x|X]] [created::2026-10-01] ^ref-x\n",
        )
        .expect("write archive");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let out = insert_ref_task(bob, &dest, line, &[]).expect("insert");
        assert_eq!(out.block_id, "ref-x-2");
        let archive = fs::read_to_string(bob.join("done/sub/note_done.md"))
            .expect("read archive");
        assert!(
            !archive.contains(&out.block_id),
            "archive must stay untouched: {archive}"
        );
    }

    #[test]
    fn renamed_source_archive_stays_reserved() {
        // The source was renamed; the archive keeps its old filename while
        // both sides name each other.
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("new.md");
        fs::write(
            &dest,
            "---\ndone_tasks: \"[[done/old_done]]\"\n---\n\n# New\n",
        )
        .expect("write");
        fs::create_dir_all(bob.join("done")).expect("done dir");
        fs::write(
            bob.join("done/old_done.md"),
            "---\nparent: \"[[new]]\"\n---\n\n- [x] #task #ref [[ref/papers/x|X]] [created::2026-10-01] ^ref-x\n",
        )
        .expect("write archive");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let out = insert_ref_task(bob, &dest, line, &[]).expect("insert");
        assert_eq!(out.block_id, "ref-x-2");
    }

    #[test]
    fn unreadable_sibling_does_not_block_missing_destination() {
        // An unreadable note elsewhere in the vault is retained as a census
        // failure, but a missing destination with no canonical archive is
        // provably unaffected, so the insert proceeds.
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        fs::write(bob.join("unreadable.md"), [0xff, 0xfe, 0x00, 0x9f])
            .expect("write unreadable");
        let dest = bob.join("mac_inbox.md");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let out = insert_ref_task(bob, &dest, line, &[]).expect("insert");
        assert_eq!(out.block_id, "ref-x");
        assert_eq!(out.placement, Placement::Created);
    }

    #[test]
    fn unreadable_sibling_poisons_involved_source() {
        // The destination names an existing archive, so a retained failure
        // could hide a dispute over that archive: the insert refuses.
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        fs::write(bob.join("unreadable.md"), [0xff, 0xfe, 0x00, 0x9f])
            .expect("write unreadable");
        let dest = bob.join("sase.md");
        fs::write(
            &dest,
            "---\ndone_tasks: \"[[done/sase_done]]\"\n---\n\n# Sase\n",
        )
        .expect("write");
        fs::create_dir_all(bob.join("done")).expect("done dir");
        fs::write(bob.join("done/sase_done.md"), "# Done\n")
            .expect("write archive");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let error =
            insert_ref_task(bob, &dest, line, &[]).expect_err("must refuse");
        assert!(error.contains("refusing"), "{error}");
        assert!(error.contains("incomplete_census"), "{error}");
    }

    #[test]
    fn reciprocal_pair_survives_unreadable_sibling() {
        // A reciprocal binding keeps its owner against unknown third
        // parties, so the reservation still applies.
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        fs::write(bob.join("unreadable.md"), [0xff, 0xfe, 0x00, 0x9f])
            .expect("write unreadable");
        let dest = bob.join("new.md");
        fs::write(
            &dest,
            "---\ndone_tasks: \"[[done/old_done]]\"\n---\n\n# New\n",
        )
        .expect("write");
        fs::create_dir_all(bob.join("done")).expect("done dir");
        fs::write(
            bob.join("done/old_done.md"),
            "---\nparent: \"[[new]]\"\n---\n\n- [x] #task #ref [[ref/papers/x|X]] [created::2026-10-01] ^ref-x\n",
        )
        .expect("write archive");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let out = insert_ref_task(bob, &dest, line, &[]).expect("insert");
        assert_eq!(out.block_id, "ref-x-2");
    }

    #[test]
    fn contradictory_binding_surfaces_as_error() {
        // `sase.md` claims an archive that reciprocally belongs to
        // `other.md`: the insertion refuses instead of silently omitting
        // the reservation.
        let temp = tempfile::tempdir().expect("temp vault");
        let bob = temp.path();
        let dest = bob.join("sase.md");
        fs::write(
            &dest,
            "---\ndone_tasks: \"[[done/other_done]]\"\n---\n\n# Sase\n",
        )
        .expect("write");
        fs::write(bob.join("other.md"), "# Other\n").expect("write other");
        fs::create_dir_all(bob.join("done")).expect("done dir");
        fs::write(
            bob.join("done/other_done.md"),
            "---\nparent: \"[[other]]\"\n---\n\n- [x] #task #ref [[ref/papers/x|X]] [created::2026-10-01] ^ref-x\n",
        )
        .expect("write archive");
        let line =
            "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let error =
            insert_ref_task(bob, &dest, line, &[]).expect_err("must refuse");
        assert!(error.contains("refusing"), "{error}");
        assert!(error.contains("contradictory_metadata"), "{error}");
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
