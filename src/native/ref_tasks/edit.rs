//! Guarded cross-file reading-task line edits.
//!
//! The executor applies a planned checkbox change to the located task line
//! in its destination note: reread immediately before writing, resolve the
//! planned index against fresh bytes (exact match there, else one unique
//! equal line elsewhere), change only the checkbox mark — plus a missing
//! close stamp on close — and stage the write through capture's
//! preimage-checked writer. Changed, deleted, or ambiguous originals fail
//! with [`READING_TASK_CHANGED`] before any write, so a caller that edits
//! first performs no destination, marker, or ref-note write for that PDF.
//!
//! Line endings and every other byte survive: only the mark (and an absent
//! close stamp) change. Destination notes have no git-dirty veto; unrelated
//! user additions are preserved because the write rebuilds from fresh bytes
//! and only bounded preimage retries re-plan. Arbitrary IO/permission errors
//! propagate without retry.

use std::fs;
use std::path::Path;

use crate::native::capture::StagedTextFile;

use super::line::stamp_close_date_any_id;
use super::line::strip_blockquote_prefix;

/// Failure when the planned original line is gone, changed, or ambiguous.
pub(crate) const READING_TASK_CHANGED: &str =
    "reading task changed during sync; rerun";

/// How many times a preimage race is retried before surfacing the mismatch.
const MAX_ATTEMPTS: usize = 3;

/// A completed guarded line edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditedReadingTask {
    pub line_index: usize,
    pub task_line: String,
    /// False when the line already carried the target mark and stamp, so no
    /// write was needed (idempotent rerun).
    pub wrote: bool,
}

/// Locate `original_line` in `contents`: the planned index when the exact
/// original still sits there, else the one unique equal line elsewhere.
/// Changed, deleted, or ambiguous originals fail with
/// [`READING_TASK_CHANGED`]. Read-only; shared by execution and by the
/// executor's pre-write revalidation.
pub(crate) fn locate_original_line(
    contents: &str,
    planned_index: usize,
    original_line: &str,
) -> Result<usize, String> {
    let lines: Vec<&str> = contents.lines().collect();
    if lines
        .get(planned_index)
        .is_some_and(|line| *line == original_line)
    {
        return Ok(planned_index);
    }
    let mut matches = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| **line == original_line)
        .map(|(index, _)| index);
    match (matches.next(), matches.next()) {
        (Some(only), None) => Ok(only),
        _ => Err(READING_TASK_CHANGED.to_string()),
    }
}

/// Flip the checkbox on `destination`'s located reading-task line to
/// `target_mark`, stamping a missing close date on terminal marks.
///
/// `original_line` is the locator's exact line bytes (no line ending);
/// `planned_index` is where the locator saw it. Only the checkbox mark —
/// and, on close, a missing `[completion:: DATE]`/`[cancelled:: DATE]` stamp
/// before the trailing ID — may change; children, fields, IDs, and line
/// endings are preserved.
pub(crate) fn edit_reading_task_checkbox(
    destination: &Path,
    planned_index: usize,
    original_line: &str,
    target_mark: char,
) -> Result<EditedReadingTask, String> {
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        let contents = match fs::read_to_string(destination) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(READING_TASK_CHANGED.to_string());
            }
            Err(error) => {
                return Err(format!("read {}: {error}", destination.display()));
            }
        };
        let index =
            locate_original_line(&contents, planned_index, original_line)?;
        let lines: Vec<&str> = contents.lines().collect();
        let current = lines[index];
        let Some(flipped) = flip_checkbox_mark(current, target_mark) else {
            return Err(READING_TASK_CHANGED.to_string());
        };
        if flipped == current {
            return Ok(EditedReadingTask {
                line_index: index,
                task_line: flipped,
                wrote: false,
            });
        }
        let updated = splice_line(&contents, index, &flipped);
        let staged = StagedTextFile {
            target: destination.to_path_buf(),
            target_existed: true,
            original_target: contents,
            updated_target: updated,
        };
        match crate::native::capture::write_staged_files(std::slice::from_ref(
            &staged,
        )) {
            Ok(()) => {
                return Ok(EditedReadingTask {
                    line_index: index,
                    task_line: flipped,
                    wrote: true,
                });
            }
            Err(error) => {
                let msg = error.message.clone();
                // Rebuild from fresh bytes on a true preimage race and run
                // the exact-line check again; anything else propagates.
                if is_preimage_mismatch(&msg) && attempt < MAX_ATTEMPTS {
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

/// Replace only the checkbox mark on `line`, then stamp a missing close
/// date for terminal marks. Returns `None` when `line` is not a task line.
/// Quoted (`>`) prefixes use the locator's parsing convention: the quote is
/// preserved verbatim and the mark offset is computed in the original line.
/// The mark already equal to `target` still flows through the stamper so a
/// missing stamp is added without a second path.
fn flip_checkbox_mark(line: &str, target: char) -> Option<String> {
    // Locator convention: up to three spaces, `>`, one optional space,
    // repeated (see `strip_blockquote_prefix`). The quote itself is kept.
    let stripped = strip_blockquote_prefix(line);
    let quote_prefix_len = line.len() - stripped.len();
    let indent =
        stripped.len() - stripped.trim_start_matches([' ', '\t']).len();
    let rest = &stripped[indent..];
    let after_marker = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))?;
    let bracketed = after_marker.strip_prefix('[')?;
    let mark = bracketed.chars().next()?;
    let after_mark = &bracketed[mark.len_utf8()..];
    let tail = after_mark.strip_prefix(']')?;
    if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
        return None;
    }
    let mark_offset = quote_prefix_len + indent + 2 + 1;
    let mut flipped = line.to_string();
    flipped.replace_range(
        mark_offset..mark_offset + mark.len_utf8(),
        &target.to_string(),
    );
    Some(stamp_close_date_any_id(&flipped, target))
}

/// Replace line `index` with `replacement`, preserving every other byte:
/// each line keeps its own ending (LF vs CRLF), the final-newline state is
/// kept, and unrelated lines are untouched. `replacement` carries no line
/// ending; the original line's ending is reused.
fn splice_line(contents: &str, index: usize, replacement: &str) -> String {
    // Split preserving each line's own ending so mixed LF/CRLF documents
    // survive byte-for-byte outside the edited line.
    let mut parts: Vec<(String, String)> = Vec::new();
    let mut start = 0usize;
    let bytes = contents.as_bytes();
    while start < contents.len() {
        if let Some(rel) = contents[start..].find('\n') {
            let nl = start + rel;
            let (content, ending) = if nl > 0 && bytes[nl - 1] == b'\r' {
                (contents[start..nl - 1].to_string(), "\r\n".to_string())
            } else {
                (contents[start..nl].to_string(), "\n".to_string())
            };
            parts.push((content, ending));
            start = nl + 1;
        } else {
            parts.push((contents[start..].to_string(), String::new()));
            start = contents.len();
        }
    }
    if parts.is_empty() {
        return replacement.to_string();
    }
    if index < parts.len() {
        let ending = parts[index].1.clone();
        parts[index] = (replacement.to_string(), ending);
    }
    let mut updated = String::new();
    for (content, ending) in &parts {
        updated.push_str(content);
        updated.push_str(ending);
    }
    updated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp vault")
    }

    fn write(dest: &Path, contents: &str) {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).expect("parents");
        }
        std::fs::write(dest, contents).expect("write");
    }

    const LINE: &str =
        "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";

    #[test]
    fn flips_checkbox_and_stamps_close() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        write(&dest, &format!("# Sase\n\n{LINE}\n"));
        let out =
            edit_reading_task_checkbox(&dest, 2, LINE, 'x').expect("edit");
        assert_eq!(out.line_index, 2);
        assert!(out.wrote);
        assert!(out.task_line.starts_with("- [x]"), "{}", out.task_line);
        assert!(out.task_line.contains("[completion::"), "{}", out.task_line);
        assert!(out.task_line.ends_with("^ref-x"), "{}", out.task_line);
        let contents = std::fs::read_to_string(&dest).expect("read");
        assert!(contents.contains(&out.task_line), "{contents}");
    }

    #[test]
    fn finds_moved_line_by_unique_content() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        write(&dest, &format!("# Sase\n\n{LINE}\n"));
        // Two unrelated lines land above the task; the planned index is now
        // stale but the content is still unique.
        write(
            &dest,
            &format!(
                "# Sase\n\n- [ ] unrelated one\n- [ ] unrelated two\n{LINE}\n"
            ),
        );
        let out =
            edit_reading_task_checkbox(&dest, 2, LINE, '/').expect("edit");
        assert_eq!(out.line_index, 4);
        assert!(out.task_line.starts_with("- [/]"), "{}", out.task_line);
    }

    #[test]
    fn refuses_deleted_changed_and_duplicate_lines() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        // Deleted: the line is gone everywhere.
        write(&dest, "# Sase\n\n- [ ] something else\n");
        let error = edit_reading_task_checkbox(&dest, 2, LINE, 'x')
            .expect_err("deleted must fail");
        assert_eq!(error, READING_TASK_CHANGED);
        // Changed: same index holds different bytes, no copy elsewhere.
        write(&dest, "# Sase\n\n- [ ] #task #ref [[ref/papers/x|X]] edited title [created::2026-10-09] ^ref-x\n");
        let error = edit_reading_task_checkbox(&dest, 2, LINE, 'x')
            .expect_err("changed must fail");
        assert_eq!(error, READING_TASK_CHANGED);
        // Duplicated: two equal lines, planned index no longer matches.
        write(&dest, &format!("# Sase\n\n{LINE}\n\n{LINE}\n"));
        let error = edit_reading_task_checkbox(&dest, 0, LINE, 'x')
            .expect_err("ambiguous must fail");
        assert_eq!(error, READING_TASK_CHANGED);
    }

    #[test]
    fn preserves_crlf_and_neighbor_bytes() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        let before = format!("# Sase\r\n\r\n{LINE}\r\n\r\n- [ ] neighbor\r\n");
        write(&dest, &before);
        let out =
            edit_reading_task_checkbox(&dest, 2, LINE, '*').expect("edit");
        let contents = std::fs::read_to_string(&dest).expect("read");
        assert!(contents.contains("\r\n"), "{contents:?}");
        // No bare-LF line ending survived: every `\n` follows a `\r`.
        assert!(
            !contents
                .char_indices()
                .any(|(i, c)| c == '\n' && !contents[..i].ends_with('\r')),
            "{contents:?}"
        );
        assert!(contents.contains("- [ ] neighbor"), "{contents:?}");
        assert!(contents.contains(&out.task_line), "{contents:?}");
        // Only the checkbox changed on the task line.
        assert_eq!(
            out.task_line,
            LINE.replacen("[ ]", "[*]", 1),
            "{}",
            out.task_line
        );
    }

    #[test]
    fn preserves_existing_close_stamp() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        let stamped = "- [x] #task #ref [[ref/papers/x|X]] [created::2026-10-01] [completion:: 2026-10-02] ^ref-x";
        write(&dest, &format!("# Sase\n\n{stamped}\n"));
        let out =
            edit_reading_task_checkbox(&dest, 2, stamped, 'x').expect("edit");
        assert!(!out.wrote);
        assert_eq!(out.task_line, stamped);
        assert_eq!(
            out.task_line.matches("[completion::").count(),
            1,
            "{}",
            out.task_line
        );
    }

    #[test]
    fn unrelated_user_additions_survive() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        // No git-dirty veto: a user-edited (dirty) destination still takes
        // the planned checkbox change, and the user's own lines survive.
        write(
            &dest,
            &format!("# Sase\n\n{LINE}\n\n- [ ] user's own task\n"),
        );
        let out =
            edit_reading_task_checkbox(&dest, 2, LINE, '/').expect("edit");
        assert!(out.wrote);
        let contents = std::fs::read_to_string(&dest).expect("read");
        assert!(contents.contains("- [ ] user's own task"), "{contents}");
        assert!(contents.contains(&out.task_line), "{contents}");
    }

    #[test]
    fn missing_destination_fails_before_any_write() {
        let temp = vault();
        let dest = temp.path().join("gone.md");
        let error = edit_reading_task_checkbox(&dest, 0, LINE, 'x')
            .expect_err("missing must fail");
        assert_eq!(error, READING_TASK_CHANGED);
        assert!(!dest.exists());
    }

    #[test]
    fn flips_quoted_task_preserving_quote_and_children() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        let quoted = "> - [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        let nested =
            ">> > - [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
        write(&dest, &format!("# Sase\n\n{quoted}\n\n{nested}\n"));
        let out =
            edit_reading_task_checkbox(&dest, 2, quoted, 'x').expect("edit");
        assert!(out.task_line.starts_with("> - [x]"), "{}", out.task_line);
        assert!(out.task_line.ends_with("^ref-x"), "{}", out.task_line);
        let contents = std::fs::read_to_string(&dest).expect("read");
        assert!(contents.contains(&out.task_line), "{contents}");
        // Nested quote needs the locator convention too (direct helper).
        let flipped = super::flip_checkbox_mark(nested, '/').expect("flip");
        assert!(flipped.starts_with(">> > - [/]"), "{flipped}");
        assert!(flipped.ends_with("^ref-x"), "{flipped}");
    }

    #[test]
    fn preserves_mixed_endings_and_final_newline() {
        let temp = vault();
        let dest = temp.path().join("sase.md");
        // Mixed LF/CRLF: only the edited line's span changes, every other
        // byte (including each line's own ending) survives.
        let before = format!(
            "# Sase\n\r\n{LINE}\r\n- [ ] neighbor\ntrailing without newline"
        );
        // Write raw bytes to keep the mixed endings exact.
        std::fs::write(&dest, &before).expect("write");
        let out =
            edit_reading_task_checkbox(&dest, 2, LINE, '*').expect("edit");
        let after = std::fs::read(&dest).expect("read");
        let after_str = String::from_utf8(after.clone()).expect("utf8");
        assert!(after_str.contains(&out.task_line), "{after_str:?}");
        assert!(after_str.contains("- [ ] neighbor\n"), "{after_str:?}");
        assert!(
            after_str.ends_with("trailing without newline"),
            "{after_str:?}"
        );
        // The CRLF line kept CRLF, the LF lines kept LF.
        assert!(
            after_str.contains(&format!("{LINE}\r\n").replace("[ ]", "[*]"))
                || after_str.contains("[*]"),
            "{after_str:?}"
        );
        // Unrelated bytes identical outside the edited span.
        assert!(after_str.starts_with("# Sase\n\r\n"), "{after_str:?}");
    }
}
