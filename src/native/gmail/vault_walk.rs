//! The vault walk shared by `bob gmail link -a` and `bob gmail sync`'s
//! watching-task scan.
//!
//! Both skip the same places: the mail folder itself, `_generated/`,
//! `_conflicts/`, `.obsidian/`, `.bob/`, `done/`, every dot directory, and
//! zorg-era generated logs. Symlinks are never followed, so a walk cannot
//! leave the vault.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use super::GmailError;
use crate::native::is_always_excluded_note_directory_name;

/// Vault directories never walked, besides the mail folder itself and any
/// dot directory.
const ALWAYS_SKIPPED_DIRS: &[&str] = &["_conflicts", "_generated", "done"];

/// Every Markdown note in the vault except the mail folder, generated and
/// state directories, the done archive, and zorg-era generated logs. Paths
/// are absolute below the canonical vault root, sorted.
pub(crate) fn scan_vault_notes(
    vault: &Path,
    folder: &Path,
) -> Result<Vec<PathBuf>, GmailError> {
    let mut notes = Vec::new();
    visit_vault_notes(vault, folder, &mut |note| {
        notes.push(note.path.to_path_buf());
        Ok(())
    })?;
    notes.sort();
    Ok(notes)
}

/// One walked note: its absolute path below the canonical vault root, the
/// vault-relative path, and its UTF-8 text.
pub(crate) struct WalkedNote<'a> {
    pub(crate) path: &'a Path,
    pub(crate) relative: &'a Path,
    pub(crate) text: &'a str,
}

/// Visit every note [`scan_vault_notes`] would return, in sorted path order,
/// with the text the walk already read. Notes that are not valid UTF-8 are
/// skipped, as they are by the link backfill.
pub(crate) fn visit_vault_notes(
    vault: &Path,
    folder: &Path,
    visit: &mut dyn FnMut(WalkedNote<'_>) -> Result<(), GmailError>,
) -> Result<(), GmailError> {
    let canonical_vault = vault.canonicalize().map_err(|_| {
        GmailError::runtime("note", "the vault path is unavailable".to_string())
    })?;
    walk(&canonical_vault, &canonical_vault, folder, visit)
}

fn walk(
    root: &Path,
    directory: &Path,
    folder: &Path,
    visit: &mut dyn FnMut(WalkedNote<'_>) -> Result<(), GmailError>,
) -> Result<(), GmailError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not scan the vault: {error}"),
        )
    })?;
    let mut entries =
        entries.collect::<Result<Vec<_>, _>>().map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not scan the vault: {error}"),
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type().map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not inspect a vault path: {error}"),
            )
        })?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if kind.is_dir() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.')
                || is_always_excluded_note_directory_name(&name)
                || ALWAYS_SKIPPED_DIRS
                    .iter()
                    .any(|skipped| OsStr::new(skipped) == name.as_os_str())
                || path_in_folder(relative, folder)
            {
                continue;
            }
            walk(root, &path, folder, visit)?;
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            if path_in_folder(relative, folder) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if is_zorg_generated(&text) {
                continue;
            }
            visit(WalkedNote {
                path: &path,
                relative,
                text: &text,
            })?;
        }
    }
    Ok(())
}

/// Whether a vault-relative path lives under the mail folder.
pub(crate) fn path_in_folder(relative: &Path, folder: &Path) -> bool {
    if folder.as_os_str().is_empty() {
        return false;
    }
    let mut folder_parts = folder.components();
    let mut relative_parts = relative.components();
    loop {
        match (folder_parts.next(), relative_parts.next()) {
            (Some(expected), Some(actual)) => {
                if expected != actual {
                    return false;
                }
            }
            (None, _) => return true,
            (Some(_), None) => return false,
        }
    }
}

/// A zorg-era generated log carries this frontmatter flag and is never
/// rewritten by a backfill or scanned for watching tasks.
pub(crate) fn is_zorg_generated(text: &str) -> bool {
    let mut lines = text.lines();
    if lines.next().is_some_and(|first| first.trim() != "---") {
        return false;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':')
            && key.trim() == "generated_from_zorg"
            && value.trim().eq_ignore_ascii_case("true")
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visit_passes_relative_paths_and_text_in_sorted_order() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let write = |relative: &str, text: &str| {
            let path = vault.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write("b.md", "bee\n");
        write("a/z.md", "zed\n");
        write("a/y.md", "why\n");
        write("_templates/t.md", "template\n");
        write(".bob/gmail/x.md", "state\n");
        write("_conflicts/c.md", "conflict\n");
        write("notes.txt", "not markdown\n");
        let mut seen = Vec::new();
        visit_vault_notes(vault, Path::new("mail"), &mut |note| {
            assert!(note.path.ends_with(note.relative));
            seen.push((
                note.relative.to_string_lossy().into_owned(),
                note.text.to_string(),
            ));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![
                ("a/y.md".to_string(), "why\n".to_string()),
                ("a/z.md".to_string(), "zed\n".to_string()),
                ("b.md".to_string(), "bee\n".to_string()),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn visit_never_follows_symlinks_out_of_the_vault() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.md"), "outside\n").unwrap();
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        fs::write(vault.join("inside.md"), "inside\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), vault.join("linked"))
            .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            vault.join("secret.md"),
        )
        .unwrap();
        let notes = scan_vault_notes(vault, Path::new("mail")).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].ends_with("inside.md"));
    }
}
