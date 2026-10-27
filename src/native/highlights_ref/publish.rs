//! Guarded public installation of a privately stamped PDF.
//!
//! Marker stamping and rendering stay in private scratch. This module is the
//! one public-install boundary: it snapshots the destination, requests a
//! bounded merging write when the destination already exists, rechecks
//! identity and marker after access, and only then runs the publication
//! callback. Companion audio belongs inside that callback.
//!
//! # Already-granted scopes
//!
//! `overwrite/execute.rs` already owns one outer file-coordination scope for
//! the filed-overwrite transaction and runs its inner marker sync through
//! [`MarkerWriteSession::for_external_coordination`]. Those helpers already
//! hold a grant; phase 4 must not wrap them in [`publish_public_pdf`].

use super::*;
use crate::native::file_coordination::Coordination;
use std::io::ErrorKind;
use std::path::Path;

/// Destination identity captured before requesting publication access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DestinationSnapshot {
    Absent,
    File {
        digest: String,
        marker: Option<String>,
    },
}

impl DestinationSnapshot {
    pub(super) fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }
}

/// Snapshot `path` without following a final symlink.
pub(super) fn capture_destination(path: &Path) -> Result<DestinationSnapshot> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(DestinationSnapshot::Absent);
        }
        Err(error) => {
            return Err(CommandError::new(format!(
                "stat destination {}: {error}",
                path.display()
            )));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(CommandError::new(format!(
            "refusing to publish over symlink {}",
            path.display()
        )));
    }
    if !metadata.file_type().is_file() {
        return Err(CommandError::new(format!(
            "unsupported destination path type: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| {
        CommandError::new(format!(
            "read destination {}: {error}",
            path.display()
        ))
    })?;
    let digest = hex::encode(Sha256::digest(&bytes));
    let marker = read_pdf_marker_from_bytes(path, &bytes)
        .ok()
        .map(|marker| marker.contents);
    Ok(DestinationSnapshot::File { digest, marker })
}

/// Run `publish` only after a grant (when coordinating an existing
/// destination) and a successful destination recheck.
///
/// Companion audio and the PDF copy belong in `publish`. A hold, coordinator
/// failure, or changed destination returns without invoking it.
pub(super) fn publish_public_pdf<T>(
    session: &MarkerWriteSession,
    dest: &Path,
    snapshot: &DestinationSnapshot,
    publish: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if !snapshot.is_absent() && session.coordinates() {
        match session.coordinate_explicit_pdf_write_with(dest, || {
            recheck_destination(dest, snapshot)?;
            publish()
        })? {
            Coordination::Granted { value, .. } => value,
            Coordination::Held(timeout) => Err(CommandError::new(format!(
                "PDF coordination timed out after {:.2}s; no files were written; close it in Highlights and retry",
                timeout.waited.as_secs_f64()
            ))
            .with_code("coordination_timeout")),
            Coordination::Failed(detail) => Err(CommandError::new(format!(
                "PDF coordination failed: {detail}; no files were written; close it in Highlights and retry"
            ))
            .with_code("coordination_failed")),
        }
    } else {
        recheck_destination(dest, snapshot)?;
        publish()
    }
}

fn recheck_destination(
    dest: &Path,
    expected: &DestinationSnapshot,
) -> Result<()> {
    let current = capture_destination(dest)?;
    match (expected, current) {
        (DestinationSnapshot::Absent, DestinationSnapshot::Absent) => Ok(()),
        (DestinationSnapshot::Absent, DestinationSnapshot::File { .. }) => {
            Err(occupied_error(dest))
        }
        (DestinationSnapshot::File { .. }, DestinationSnapshot::Absent) => {
            Err(preimage_changed_error(dest))
        }
        (
            DestinationSnapshot::File {
                digest: expected_digest,
                marker: expected_marker,
            },
            DestinationSnapshot::File {
                digest: current_digest,
                marker: current_marker,
            },
        ) => {
            if expected_digest == &current_digest {
                return Ok(());
            }
            if markers_differ(expected_marker, &current_marker) {
                Err(marker_changed_error(dest))
            } else {
                Err(preimage_changed_error(dest))
            }
        }
    }
}

fn markers_differ(expected: &Option<String>, current: &Option<String>) -> bool {
    match (expected, current) {
        (Some(expected), Some(current)) => {
            normalize_line_endings(expected) != normalize_line_endings(current)
        }
        (None, None) => false,
        _ => true,
    }
}

fn occupied_error(dest: &Path) -> CommandError {
    CommandError::new(format!(
        "destination appeared while publication waited: {}; no files were written",
        dest.display()
    ))
    .with_code("preimage_changed")
}

fn preimage_changed_error(dest: &Path) -> CommandError {
    CommandError::new(format!(
        "Highlights or another writer changed the destination PDF while publication waited: {}; no files were written. Close it in Highlights and retry",
        dest.display()
    ))
    .with_code("preimage_changed")
}

fn marker_changed_error(dest: &Path) -> CommandError {
    CommandError::new(format!(
        "marker changed in Highlights during publication: {}; no files were written; close it in Highlights and retry",
        dest.display()
    ))
    .with_code("marker_changed")
}

/// Copy `source` onto an absent `dest`. An intervening occupant is a
/// refusal, never a replacement.
pub(super) fn copy_pdf_no_clobber(source: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let temp_path = temporary_write_path(dest)?;
    let _ = fs::remove_file(&temp_path);
    fs::copy(source, &temp_path).map_err(|error| {
        CommandError::new(format!(
            "copy PDF {} to temporary file {}: {error}",
            source.display(),
            temp_path.display()
        ))
    })?;
    fs::File::open(&temp_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            let _ = fs::remove_file(&temp_path);
            CommandError::new(format!(
                "sync temporary file {}: {error}",
                temp_path.display()
            ))
        })?;
    match rename_no_replace(&temp_path, dest) {
        Ok(()) => {
            if let Some(parent) = dest.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::File::open(parent)
                    .and_then(|file| file.sync_all())
                    .map_err(|error| {
                        CommandError::new(format!(
                            "sync directory {}: {error}",
                            parent.display()
                        ))
                    })?;
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temp_path);
            Err(occupied_error(dest))
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(CommandError::new(format!(
                "install PDF {}: {error}",
                dest.display()
            )))
        }
    }
}
