//! Shared annotated-archive identity, allocation, and metadata reading.
//!
//! Guard and recovery consume this module without editing `overwrite.rs`.
//! Paths are vault-relative, stay under `ref_history/`, and belong to the
//! live reference named by each frozen note's `parent` plus the live
//! `earlier:` list.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::native::ref_library::{lineage, ParsedFrontmatter};

use super::super::split_frontmatter;

/// One collision-free archive stem and its companion destinations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveBundle {
    /// Vault-relative stem without extension:
    /// `ref_history/<type>/<stem>__<date>` or `..._<n>`.
    pub(crate) stem: String,
    pub(crate) note_path: String,
    pub(crate) annotated_pdf_path: String,
    pub(crate) assets_path: String,
}

/// Note-local evidence for one frozen annotated capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchivePdf {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

/// A frozen capture that belongs to one live reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveRecord {
    pub(crate) note_path: String,
    pub(crate) parent: String,
    pub(crate) annotated_pdf: Option<ArchivePdf>,
    pub(crate) assets_path: Option<String>,
}

/// Failures while reading live lineage or a frozen archive note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveReadError {
    pub(crate) message: String,
}

/// Reserve one stem across the note, annotated PDF, and assets companions.
pub(crate) fn allocate_bundle(
    vault_root: &Path,
    ref_type: &str,
    live_stem: &str,
    date: &str,
) -> ArchiveBundle {
    let parent = format!("ref_history/{ref_type}");
    let base = format!("{parent}/{live_stem}__{date}");
    for suffix in 1u32.. {
        let stem = if suffix == 1 {
            base.clone()
        } else {
            format!("{base}_{suffix}")
        };
        if !stem_occupied(vault_root, &stem) {
            return bundle_for_stem(stem);
        }
    }
    unreachable!("archive suffix space is unbounded")
}

fn bundle_for_stem(stem: String) -> ArchiveBundle {
    ArchiveBundle {
        note_path: format!("{stem}.md"),
        annotated_pdf_path: format!("{stem}__annotated.pdf"),
        assets_path: format!("{stem}.assets"),
        stem,
    }
}

fn stem_occupied(vault_root: &Path, stem: &str) -> bool {
    path_occupied(&vault_root.join(format!("{stem}.md")))
        || path_occupied(&vault_root.join(format!("{stem}__annotated.pdf")))
        || path_occupied(&vault_root.join(format!("{stem}.assets")))
}

fn path_occupied(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Resolve frozen archives named by a live note's `earlier:` list.
///
/// Entries whose paths are unsafe, whose frozen `parent` does not name
/// this live note, or whose annotated PDF is not a sibling of the frozen
/// note are skipped. Filename guesses and hashes on unrelated notes never
/// invent membership.
pub(crate) fn records_for_live_note(
    vault_root: &Path,
    live_note_path: &str,
) -> Result<Vec<ArchiveRecord>, ArchiveReadError> {
    let live_absolute = vault_root.join(live_note_path);
    let contents = fs::read_to_string(&live_absolute).map_err(|error| {
        ArchiveReadError {
            message: format!("could not read {live_note_path}: {error}"),
        }
    })?;
    let Some((front, _)) = split_frontmatter(&contents) else {
        return Ok(Vec::new());
    };
    let parsed = ParsedFrontmatter::parse(&front);
    let live_identity = live_parent_identity(live_note_path);
    let mut records = Vec::new();
    for entry in lineage::parse_earlier(&parsed) {
        if entry.kind != lineage::EarlierKind::Replaced {
            continue;
        }
        let Some(record) =
            record_from_target(vault_root, &entry.target, &live_identity)?
        else {
            continue;
        };
        records.push(record);
    }
    Ok(records)
}

fn live_parent_identity(live_note_path: &str) -> String {
    let path = vault_slash(live_note_path);
    let without_ext = path
        .strip_suffix(".md")
        .or_else(|| path.strip_suffix(".MD"))
        .unwrap_or(&path);
    format!("[[{without_ext}]]")
}

fn record_from_target(
    vault_root: &Path,
    target: &str,
    live_identity: &str,
) -> Result<Option<ArchiveRecord>, ArchiveReadError> {
    let Some(note_path) = archive_note_path(target) else {
        return Ok(None);
    };
    let Some(absolute) = validated_archive_file(vault_root, &note_path)? else {
        return Ok(None);
    };
    let contents =
        fs::read_to_string(&absolute).map_err(|error| ArchiveReadError {
            message: format!("could not read {note_path}: {error}"),
        })?;
    let Some((front, _)) = split_frontmatter(&contents) else {
        return Ok(None);
    };
    let parsed = ParsedFrontmatter::parse(&front);
    let Some(parent) = parsed.get_str("parent") else {
        return Ok(None);
    };
    if !parents_match(parent, live_identity) {
        return Ok(None);
    }
    let annotated_pdf = annotated_pdf_from_front(
        vault_root,
        &note_path,
        parsed.get_str("annotated_pdf"),
        parsed.get_str("annotated_pdf_sha256"),
    )?;
    let assets_path = sibling_assets_path(vault_root, &note_path)?;
    Ok(Some(ArchiveRecord {
        note_path,
        parent: parent.to_string(),
        annotated_pdf,
        assets_path,
    }))
}

fn archive_note_path(target: &str) -> Option<String> {
    let path = vault_slash(target.trim());
    let path = path.strip_prefix("./").unwrap_or(&path);
    if path.is_empty() {
        return None;
    }
    let with_ext = if path.ends_with(".md") || path.ends_with(".MD") {
        path.to_string()
    } else {
        format!("{path}.md")
    };
    with_ext.starts_with("ref_history/").then_some(with_ext)
}

fn annotated_pdf_from_front(
    vault_root: &Path,
    note_path: &str,
    raw_pdf: Option<&str>,
    raw_hash: Option<&str>,
) -> Result<Option<ArchivePdf>, ArchiveReadError> {
    let Some(raw_pdf) = raw_pdf else {
        return Ok(None);
    };
    let Some(hash) = raw_hash.filter(|value| is_sha256(value)) else {
        return Ok(None);
    };
    let Some(pdf_path) = wikilink_or_path(raw_pdf) else {
        return Ok(None);
    };
    if !pdf_belongs_to_note(note_path, &pdf_path) {
        return Ok(None);
    }
    // The recorded digest is immutable evidence even if the archived file
    // has since gone missing. Validate that its path is still safe and
    // belongs to this archive, but do not require the file to be present.
    if validated_archive_path(vault_root, &pdf_path)?.is_none() {
        return Ok(None);
    }
    Ok(Some(ArchivePdf {
        path: pdf_path,
        sha256: hash.to_string(),
    }))
}

fn sibling_assets_path(
    vault_root: &Path,
    note_path: &str,
) -> Result<Option<String>, ArchiveReadError> {
    let stem = note_path
        .strip_suffix(".md")
        .or_else(|| note_path.strip_suffix(".MD"))
        .unwrap_or(note_path);
    let assets_path = format!("{stem}.assets");
    match validated_archive_path(vault_root, &assets_path)? {
        Some(_) if path_occupied(&vault_root.join(&assets_path)) => {
            Ok(Some(assets_path))
        }
        _ => Ok(None),
    }
}

fn pdf_belongs_to_note(note_path: &str, pdf_path: &str) -> bool {
    let note_stem = note_path
        .strip_suffix(".md")
        .or_else(|| note_path.strip_suffix(".MD"))
        .unwrap_or(note_path);
    let expected = format!("{note_stem}__annotated.pdf");
    vault_slash(pdf_path).eq_ignore_ascii_case(&expected)
}

fn parents_match(frozen_parent: &str, live_identity: &str) -> bool {
    normalize_parent(frozen_parent) == normalize_parent(live_identity)
}

fn normalize_parent(value: &str) -> String {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .unwrap_or(trimmed);
    let inner = inner.split('|').next().unwrap_or(inner).trim();
    let inner = inner.split('#').next().unwrap_or(inner).trim();
    let inner = inner.strip_prefix("./").unwrap_or(inner);
    let inner = inner
        .strip_suffix(".md")
        .or_else(|| inner.strip_suffix(".MD"))
        .unwrap_or(inner);
    vault_slash(inner).to_ascii_lowercase()
}

fn wikilink_or_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .unwrap_or(trimmed);
    let inner = inner.split('|').next().unwrap_or(inner).trim();
    let inner = inner.split('#').next().unwrap_or(inner).trim();
    let inner = inner.strip_prefix("./").unwrap_or(inner);
    let path = vault_slash(inner);
    (!path.is_empty()).then_some(path)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn validated_archive_file(
    vault_root: &Path,
    relative: &str,
) -> Result<Option<PathBuf>, ArchiveReadError> {
    let Some(path) = validated_archive_path(vault_root, relative)? else {
        return Ok(None);
    };
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(path)),
        _ => Ok(None),
    }
}

fn validated_archive_path(
    vault_root: &Path,
    relative: &str,
) -> Result<Option<PathBuf>, ArchiveReadError> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Ok(None);
    }
    let mut parts = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str());
    if parts.next() != Some("ref_history") {
        return Ok(None);
    }
    let root = vault_root
        .canonicalize()
        .map_err(|error| ArchiveReadError {
            message: format!("resolve vault root: {error}"),
        })?;
    let mut current = vault_root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Ok(None);
        };
        current.push(name);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && index + 1 == components.len() =>
            {
                let Some(parent) = current.parent() else {
                    return Ok(None);
                };
                return match parent.canonicalize() {
                    Ok(resolved_parent)
                        if resolved_parent.starts_with(&root) =>
                    {
                        Ok(Some(current))
                    }
                    _ => Ok(None),
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => {
                return Err(ArchiveReadError {
                    message: format!("inspect {}: {error}", current.display()),
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Ok(None);
        }
    }
    match current.canonicalize() {
        Ok(resolved) if resolved.starts_with(&root) => Ok(Some(current)),
        _ => Ok(None),
    }
}

fn vault_slash(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn frozen_note(
        live: &str,
        pdf: Option<&str>,
        hash: Option<&str>,
    ) -> String {
        let pdf_lines = match (pdf, hash) {
            (Some(pdf), Some(hash)) => format!(
                "annotated_pdf: \"[[{pdf}]]\"\nannotated_pdf_sha256: {hash}\n"
            ),
            _ => String::new(),
        };
        format!(
            "---\ntitle: Paper\nparent: \"[[{live}]]\"\nreplaced: 2026-10-25\n{pdf_lines}---\n# Paper\n"
        )
    }

    #[test]
    fn allocation_skips_each_companion_and_same_day_collisions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let first = allocate_bundle(root, "papers", "paper", "2026-10-25");
        assert_eq!(first.stem, "ref_history/papers/paper__2026-10-25");
        assert_eq!(first.note_path, "ref_history/papers/paper__2026-10-25.md");
        assert_eq!(
            first.annotated_pdf_path,
            "ref_history/papers/paper__2026-10-25__annotated.pdf"
        );
        assert_eq!(
            first.assets_path,
            "ref_history/papers/paper__2026-10-25.assets"
        );

        write(&root.join(&first.note_path), "note");
        let after_note = allocate_bundle(root, "papers", "paper", "2026-10-25");
        assert_eq!(after_note.stem, "ref_history/papers/paper__2026-10-25_2");

        fs::remove_file(root.join(&first.note_path)).unwrap();
        write(&root.join(&first.annotated_pdf_path), "pdf");
        let after_pdf = allocate_bundle(root, "papers", "paper", "2026-10-25");
        assert_eq!(after_pdf.stem, "ref_history/papers/paper__2026-10-25_2");

        fs::remove_file(root.join(&first.annotated_pdf_path)).unwrap();
        fs::create_dir_all(root.join(&first.assets_path)).unwrap();
        let after_assets =
            allocate_bundle(root, "papers", "paper", "2026-10-25");
        assert_eq!(after_assets.stem, "ref_history/papers/paper__2026-10-25_2");

        write(
            &root.join("ref_history/papers/paper__2026-10-25_2.md"),
            "second",
        );
        let third = allocate_bundle(root, "papers", "paper", "2026-10-25");
        assert_eq!(third.stem, "ref_history/papers/paper__2026-10-25_3");
    }

    #[test]
    fn reader_uses_earlier_and_parent_not_unrelated_hashes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let hash = "ab".repeat(32);
        let other_hash = "cd".repeat(32);
        write(
            &root.join("ref_history/papers/paper__2026-10-25.md"),
            &frozen_note(
                "ref/papers/paper",
                Some("ref_history/papers/paper__2026-10-25__annotated.pdf"),
                Some(&hash),
            ),
        );
        write(
            &root.join("ref_history/papers/paper__2026-10-25__annotated.pdf"),
            "pdf-bytes",
        );
        fs::create_dir_all(
            root.join("ref_history/papers/paper__2026-10-25.assets"),
        )
        .unwrap();
        write(
            &root.join("ref_history/papers/other__2026-10-25.md"),
            &frozen_note(
                "ref/papers/other",
                Some("ref_history/papers/other__2026-10-25__annotated.pdf"),
                Some(&other_hash),
            ),
        );
        write(
            &root.join("ref_history/papers/other__2026-10-25__annotated.pdf"),
            "other-pdf",
        );
        write(
            &root.join("ref/papers/paper.md"),
            "---\ntitle: Paper\nearlier:\n  - \"[[ref_history/papers/paper__2026-10-25|replaced 2026-10-25 · 2 highlights]]\"\n  - \"[[ref_history/papers/other__2026-10-25|replaced 2026-10-25 · 1 highlight]]\"\n---\n# Paper\n",
        );

        let records =
            records_for_live_note(root, "ref/papers/paper.md").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].note_path,
            "ref_history/papers/paper__2026-10-25.md"
        );
        assert_eq!(records[0].parent, "[[ref/papers/paper]]");
        let pdf = records[0].annotated_pdf.as_ref().unwrap();
        assert_eq!(
            pdf.path,
            "ref_history/papers/paper__2026-10-25__annotated.pdf"
        );
        assert_eq!(pdf.sha256, hash);
        assert_eq!(
            records[0].assets_path.as_deref(),
            Some("ref_history/papers/paper__2026-10-25.assets")
        );
        fs::remove_file(
            root.join("ref_history/papers/paper__2026-10-25__annotated.pdf"),
        )
        .unwrap();
        let records =
            records_for_live_note(root, "ref/papers/paper.md").unwrap();
        assert_eq!(
            records[0]
                .annotated_pdf
                .as_ref()
                .map(|pdf| pdf.sha256.as_str()),
            Some(hash.as_str()),
            "the safe recorded digest survives a missing archived PDF"
        );
    }

    #[test]
    fn reader_skips_unsafe_and_mismatched_archive_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            &root.join("ref_history/papers/paper__2026-10-25.md"),
            &frozen_note("ref/papers/paper", None, None),
        );
        write(
            &root.join("outside.md"),
            &frozen_note("ref/papers/paper", None, None),
        );
        write(
            &root.join("ref/papers/paper.md"),
            "---\ntitle: Paper\nearlier:\n  - \"[[ref_history/../outside]]\"\n  - \"[[lib/papers/paper.pdf]]\"\n  - \"[[ref_history/papers/missing__2026-10-25]]\"\n---\n# Paper\n",
        );
        let records =
            records_for_live_note(root, "ref/papers/paper.md").unwrap();
        assert!(records.is_empty());
    }
}
