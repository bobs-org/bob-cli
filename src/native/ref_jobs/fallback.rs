//! Lossless inbox-task fallback writer.
//!
//! When a clip fails, the link becomes exactly the task line capture
//! would have written with routing off, plus one `note` child bullet
//! (the `⚠️ Clip failed …` text). It inserts through capture's
//! staged-file commit — same section placement as capture, same
//! disk-preimage guard with re-read retries — and never takes the
//! vault lock, matching capture.

use std::{fs, path::Path};

use crate::native::capture::{
    dominant_indent_unit, insert_task_line, line_spans, validate_target_parent,
    write_staged_files, Placement, StagedTextFile,
};

/// Fallback attempts: one try plus re-reads on preimage conflicts.
const FALLBACK_ATTEMPTS: usize = 3;

/// `task_line` plus one indented `- note` child bullet.
pub(crate) fn fallback_block(
    task_line: &str,
    note: &str,
    indent_unit: &str,
) -> String {
    format!("{task_line}\n{indent_unit}- {note}")
}

/// Append the fallback block for a failed clip to `relative_target`
/// under `bob_dir`, creating the target exactly as capture would.
/// Returns capture's placement for the write.
pub(crate) fn write_fallback(
    bob_dir: &Path,
    relative_target: &str,
    task_line: &str,
    note: &str,
) -> Result<Placement, String> {
    write_fallback_with_hook(bob_dir, relative_target, task_line, note, None)
}

/// [`write_fallback`] with a test hook run after each read, before
/// the commit: a hook that edits the target proves the preimage
/// retry re-reads and still commits exactly once.
fn write_fallback_with_hook(
    bob_dir: &Path,
    relative_target: &str,
    task_line: &str,
    note: &str,
    disturb: Option<&dyn Fn()>,
) -> Result<Placement, String> {
    let target = bob_dir.join(relative_target);
    let mut last_error = String::new();
    for _ in 0..FALLBACK_ATTEMPTS {
        let existed = fs::symlink_metadata(&target).is_ok();
        let current = if existed {
            match fs::read_to_string(&target) {
                Ok(contents) => contents,
                Err(error) => {
                    return Err(format!(
                        "read fallback target {}: {error}",
                        target.display()
                    ));
                }
            }
        } else {
            if let Err(error) = validate_target_parent(&target) {
                return Err(error.message);
            }
            String::new()
        };
        let indent =
            dominant_indent_unit(&line_spans(&current)).unwrap_or("\t");
        let block = fallback_block(task_line, note, indent);
        let (updated, placement) = insert_task_line(&current, &block);
        let staged = StagedTextFile {
            target: target.clone(),
            target_existed: existed,
            original_target: current,
            updated_target: updated,
        };
        if let Some(disturb) = disturb {
            disturb();
        }
        match write_staged_files(std::slice::from_ref(&staged)) {
            Ok(()) => return Ok(placement),
            Err(error) if error.message.contains("after planning") => {
                // Re-read and retry with a fresh preimage.
                last_error = error.message;
            }
            Err(error) => return Err(error.message),
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_vault(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bob-cli-ref-jobs-fallback-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create vault");
        dir
    }

    #[test]
    fn fallback_creates_missing_target_like_capture() {
        let vault = temp_vault("missing");
        write_fallback(
            &vault,
            "mac_inbox.md",
            "- [ ] #task https://example.com/post [created::2026-10-07]",
            "⚠️ Clip failed (blocked): nope · retry: bob ref create https://example.com/post",
        )
        .expect("fallback write");
        let contents = fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("read target");
        let (expected, _) = insert_task_line(
            "",
            &fallback_block(
                "- [ ] #task https://example.com/post [created::2026-10-07]",
                "⚠️ Clip failed (blocked): nope · retry: bob ref create https://example.com/post",
                "\t",
            ),
        );
        assert_eq!(contents, expected);
    }

    #[test]
    fn fallback_appends_under_existing_tasks_with_file_indent() {
        let vault = temp_vault("existing");
        fs::write(
            vault.join("mac_inbox.md"),
            "## Tasks\n\n- [ ] #task first\n  - child\n",
        )
        .expect("seed target");
        write_fallback(
            &vault,
            "mac_inbox.md",
            "- [ ] #task https://example.com/post [created::2026-10-07]",
            "⚠️ Clip failed (timeout): slow · retry: bob ref create https://example.com/post",
        )
        .expect("fallback write");
        let contents = fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("read target");
        assert!(
            contents.contains(
                "- [ ] #task https://example.com/post [created::2026-10-07]\n  - ⚠️ Clip failed (timeout)"
            ),
            "{contents}"
        );
    }

    #[test]
    fn fallback_refuses_a_missing_vault_root() {
        let vault = temp_vault("noroot");
        let error = write_fallback(
            &vault.join("nope"),
            "mac_inbox.md",
            "- [ ] #task https://example.com/post [created::2026-10-07]",
            "⚠️ Clip failed (blocked): nope · retry: bob ref create https://example.com/post",
        )
        .expect_err("missing root must fail");
        assert!(error.contains("does not exist"), "{error}");
    }

    #[test]
    fn concurrent_edit_retries_and_commits_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let vault = temp_vault("retry");
        fs::write(vault.join("mac_inbox.md"), "one\n").expect("seed");
        let edits = AtomicUsize::new(0);
        write_fallback_with_hook(
            &vault,
            "mac_inbox.md",
            "- [ ] #task https://example.com/post [created::2026-10-07]",
            "⚠️ Clip failed (blocked): nope · retry: bob ref create https://example.com/post",
            Some(&|| {
                // Disturb only the first attempt: the commit must fail
                // its preimage check, re-read, and commit on retry.
                if edits.fetch_add(1, Ordering::SeqCst) == 0 {
                    let mut contents =
                        fs::read_to_string(vault.join("mac_inbox.md"))
                            .expect("read target");
                    contents.push_str("two\n");
                    fs::write(vault.join("mac_inbox.md"), contents)
                        .expect("disturb target");
                }
            }),
        )
        .expect("retry commits");
        assert_eq!(edits.load(Ordering::SeqCst), 2, "one conflict, one commit");
        let contents = fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("read target");
        assert!(
            contents.contains("two\n")
                && contents.contains("- [ ] #task https://example.com/post [created::2026-10-07]")
                && contents.contains("⚠️ Clip failed (blocked)"),
            "{contents}"
        );
        assert_eq!(
            contents
                .matches("- [ ] #task https://example.com/post")
                .count(),
            1,
            "exactly one fallback block: {contents}"
        );
    }

    #[test]
    fn stale_preimage_is_a_conflict_error() {
        let vault = temp_vault("stale");
        fs::write(vault.join("mac_inbox.md"), "one\n").expect("seed");
        let staged = StagedTextFile {
            target: vault.join("mac_inbox.md"),
            target_existed: true,
            original_target: "stale\n".to_string(),
            updated_target: "stale\nnew\n".to_string(),
        };
        let error = write_staged_files(std::slice::from_ref(&staged))
            .expect_err("stale preimage must fail");
        assert!(
            error.message.contains("after planning"),
            "{}",
            error.message
        );
        // The real write still commits afterwards: the retry path
        // recovers from exactly this state.
        write_fallback(
            &vault,
            "mac_inbox.md",
            "- [ ] #task https://example.com/post [created::2026-10-07]",
            "⚠️ Clip failed (blocked): nope · retry: bob ref create https://example.com/post",
        )
        .expect("retry recovers");
    }
}
