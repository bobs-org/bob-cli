use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

use super::model::{
    CaptureError, FileIdentity, InputKind, InputSnapshot, InputState,
    PlannedWrite,
};

#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0o400000;

pub(crate) fn capture_required(
    path: &Path,
    kind: InputKind,
) -> Result<InputSnapshot, CaptureError> {
    match capture_optional(path, kind)? {
        snapshot if snapshot.is_missing() => {
            Err(CaptureError::NotFound(path.to_path_buf()))
        }
        snapshot => Ok(snapshot),
    }
}

pub(crate) fn capture_optional(
    path: &Path,
    kind: InputKind,
) -> Result<InputSnapshot, CaptureError> {
    match capture_present(path, kind) {
        Ok(snapshot) => Ok(snapshot),
        Err(CaptureError::NotFound(_)) => {
            Ok(InputSnapshot::missing(path, kind))
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn planned_write(
    path: PathBuf,
    original: &InputSnapshot,
    proposed_bytes: Vec<u8>,
    structural_regrouping: bool,
) -> Result<PlannedWrite, CaptureError> {
    reject_multiply_linked(&path, original, "replace")?;
    Ok(PlannedWrite {
        path,
        original: original.state.clone(),
        proposed_bytes: Some(proposed_bytes),
        structural_regrouping,
    })
}

/// Build an explicit removal. Archive rename-following is the first
/// production caller; unit tests exercise the writer until that lands.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn planned_remove(
    path: PathBuf,
    original: &InputSnapshot,
) -> Result<PlannedWrite, CaptureError> {
    if original.is_missing() {
        return Err(CaptureError::NotFound(path));
    }
    reject_multiply_linked(&path, original, "remove")?;
    Ok(PlannedWrite {
        path,
        original: original.state.clone(),
        proposed_bytes: None,
        structural_regrouping: false,
    })
}

fn reject_multiply_linked(
    path: &Path,
    original: &InputSnapshot,
    verb: &str,
) -> Result<(), CaptureError> {
    match &original.state {
        InputState::Present { identity, .. } if identity.nlink != 1 => {
            Err(CaptureError::Unsupported {
                path: path.to_path_buf(),
                message: format!("refusing to {verb} a multiply linked file"),
            })
        }
        _ => Ok(()),
    }
}

pub(crate) fn snapshot_for_path<'a>(
    inputs: &'a [InputSnapshot],
    path: &Path,
) -> Option<&'a InputSnapshot> {
    let canonical = path.canonicalize().ok();
    inputs.iter().find(|input| {
        if input.path == path {
            return true;
        }
        match (&canonical, input.identity()) {
            (Some(canonical), Some(identity)) => {
                identity.canonical_path == *canonical
            }
            _ => false,
        }
    })
}

pub(super) fn recapture(
    input: &InputSnapshot,
) -> Result<InputSnapshot, CaptureError> {
    match input.state {
        InputState::Missing => capture_optional(&input.path, input.kind),
        InputState::Present { .. } => capture_required(&input.path, input.kind),
    }
}

fn capture_present(
    path: &Path,
    kind: InputKind,
) -> Result<InputSnapshot, CaptureError> {
    let first = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CaptureError::NotFound(path.to_path_buf()));
        }
        Err(error) => return Err(CaptureError::io(path, error)),
    };
    reject_non_regular(path, &first)?;
    let bytes = read_regular_file(path)?;
    let second = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CaptureError::Unstable(path.to_path_buf()));
        }
        Err(error) => return Err(CaptureError::io(path, error)),
    };
    reject_non_regular(path, &second)?;
    if metadata_fingerprint(&first) != metadata_fingerprint(&second) {
        return Err(CaptureError::Unstable(path.to_path_buf()));
    }
    let identity = file_identity(path, &second)?;
    Ok(InputSnapshot {
        path: path.to_path_buf(),
        kind,
        state: InputState::Present { identity, bytes },
    })
}

fn reject_non_regular(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), CaptureError> {
    if metadata.file_type().is_symlink() {
        return Err(CaptureError::Unsupported {
            path: path.to_path_buf(),
            message: "path is a symlink".to_string(),
        });
    }
    if !metadata.file_type().is_file() {
        return Err(CaptureError::Unsupported {
            path: path.to_path_buf(),
            message: "path is not a regular file".to_string(),
        });
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, CaptureError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CaptureError::Unstable(path.to_path_buf())
        } else {
            CaptureError::io(path, error)
        }
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| CaptureError::io(path, error))?;
    Ok(bytes)
}

fn file_identity(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<FileIdentity, CaptureError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let canonical_path = fs::canonicalize(path)
            .map_err(|error| CaptureError::io(path, error))?;
        Ok(FileIdentity {
            canonical_path,
            dev: metadata.dev(),
            ino: metadata.ino(),
            nlink: metadata.nlink(),
            mode: metadata.mode() & 0o7777,
            len: metadata.len(),
            mtime: metadata.modified().ok(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(CaptureError::Unsupported {
            path: path.to_path_buf(),
            message: "guarded writes require Unix file identity".to_string(),
        })
    }
}

fn metadata_fingerprint(
    metadata: &fs::Metadata,
) -> (u64, u64, u64, u32, u64, Option<SystemTime>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.nlink(),
            metadata.mode(),
            metadata.len(),
            metadata.modified().ok(),
        )
    }
    #[cfg(not(unix))]
    {
        (0, 0, 0, 0, metadata.len(), metadata.modified().ok())
    }
}
