//! Validated attachment snapshots for compose bodies.
//!
//! Declared vault-relative attachment paths are validated against the vault,
//! read once, and snapshotted into a private staging directory. The later
//! transport phase must read these snapshots — never the race-prone originals —
//! and transmit each file under its original basename together with the
//! recorded byte length and SHA-256.
//!
//! The staging directory is created mode 0700 and every snapshot file is
//! mode 0600. Dropping the staging value removes the whole directory, so
//! snapshots never survive success or error and never become a durable body
//! cache.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

/// One validated attachment snapshot ready for transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedAttachment {
    /// Declared vault-relative path, slash-joined for digests and previews.
    pub(crate) vault_path: PathBuf,
    /// Transmitted file name: the original basename.
    pub(crate) filename: String,
    /// Private snapshot file the transport phase must read.
    pub(crate) snapshot: PathBuf,
    /// Exact byte length of the snapshot.
    pub(crate) len: u64,
    /// SHA-256 of the exact snapshot bytes, lowercase hex.
    pub(crate) sha256: String,
}

/// Private staging directory plus its validated snapshots.
///
/// Dropping this value removes the staging directory and every snapshot.
pub(crate) struct AttachmentStaging {
    dir: tempfile::TempDir,
    files: Vec<PreparedAttachment>,
}

impl std::fmt::Debug for AttachmentStaging {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachmentStaging")
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

/// Attachment validation or staging failure. Messages name paths, never body
/// bytes, so they stay safe for errors, logs, and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachmentError {
    UnsafePath(PathBuf),
    Missing(PathBuf),
    NotAFile(PathBuf),
    Unreadable(PathBuf, String),
    Unstageable(String),
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafePath(path) => write!(
                f,
                "attachment {} is not a safe vault-relative path",
                path.display()
            ),
            Self::Missing(path) => {
                write!(f, "attachment {} does not exist", path.display())
            }
            Self::NotAFile(path) => {
                write!(f, "attachment {} is not a regular file", path.display())
            }
            Self::Unreadable(path, detail) => {
                write!(f, "attachment {}: {detail}", path.display())
            }
            Self::Unstageable(detail) => {
                write!(f, "could not stage attachments: {detail}")
            }
        }
    }
}

impl std::error::Error for AttachmentError {}

impl AttachmentStaging {
    /// Validate `declared` vault-relative paths and snapshot their bytes.
    ///
    /// Identical paths attach once. Snapshots are written in declared order
    /// under deterministic `NNN_` names so repeated runs agree.
    pub(crate) fn prepare(
        vault: &Path,
        declared: &[PathBuf],
    ) -> Result<Self, AttachmentError> {
        let dir = tempfile::TempDir::new().map_err(|error| {
            AttachmentError::Unstageable(format!(
                "private staging failed: {error}"
            ))
        })?;
        restrict_dir(dir.path()).map_err(AttachmentError::Unstageable)?;
        let mut files = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        for (index, path) in declared.iter().enumerate() {
            if seen.contains(path) {
                continue;
            }
            seen.push(path.clone());
            let bytes = read_validated(vault, path)?;
            let mut hash = Sha256::new();
            hash.update(&bytes);
            let sha256 = hex::encode(hash.finalize());
            let filename = attachment_filename(path);
            let snapshot = dir.path().join(format!("{index:03}_{filename}"));
            fs::write(&snapshot, &bytes).map_err(|error| {
                AttachmentError::Unreadable(
                    path.clone(),
                    format!("could not snapshot bytes: {error}"),
                )
            })?;
            restrict_file(&snapshot).map_err(|detail| {
                AttachmentError::Unreadable(path.clone(), detail)
            })?;
            // Re-reading proves gog sees the hashed bytes, not the original.
            let staged = fs::read(&snapshot).map_err(|error| {
                AttachmentError::Unreadable(
                    path.clone(),
                    format!("could not verify snapshot: {error}"),
                )
            })?;
            if staged != bytes {
                return Err(AttachmentError::Unreadable(
                    path.clone(),
                    "snapshot bytes changed during staging".to_string(),
                ));
            }
            files.push(PreparedAttachment {
                vault_path: path.clone(),
                filename,
                snapshot,
                len: bytes.len() as u64,
                sha256,
            });
        }
        Ok(Self { dir, files })
    }

    /// Private staging directory holding the snapshots.
    pub(crate) fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// Snapshots in declared order.
    pub(crate) fn files(&self) -> &[PreparedAttachment] {
        &self.files
    }
}

fn attachment_filename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Read a declared attachment after validating every path component.
///
/// Refuses empty, absolute, and non-`Normal` components (including parent
/// references), control characters, symlinks anywhere along the vault path,
/// missing files, and non-regular files.
fn read_validated(
    vault: &Path,
    path: &Path,
) -> Result<Vec<u8>, AttachmentError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.to_string_lossy().chars().any(char::is_control)
    {
        return Err(AttachmentError::UnsafePath(path.to_path_buf()));
    }
    let mut current = vault.to_path_buf();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AttachmentError::UnsafePath(path.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(AttachmentError::Missing(path.to_path_buf()));
            }
            Err(error) => {
                return Err(AttachmentError::Unreadable(
                    path.to_path_buf(),
                    format!("could not inspect path: {error}"),
                ));
            }
        }
    }
    let target = vault.join(path);
    let metadata = fs::symlink_metadata(&target).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AttachmentError::Missing(path.to_path_buf())
        } else {
            AttachmentError::Unreadable(
                path.to_path_buf(),
                format!("could not inspect file: {error}"),
            )
        }
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AttachmentError::NotAFile(path.to_path_buf()));
    }
    fs::read(&target).map_err(|error| {
        AttachmentError::Unreadable(
            path.to_path_buf(),
            format!("could not read file: {error}"),
        )
    })
}

fn restrict_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(
            |error| format!("could not restrict staging dir: {error}"),
        )?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

fn restrict_file(file: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).map_err(
            |error| format!("could not restrict snapshot file: {error}"),
        )?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let vault = tempfile::TempDir::new().expect("vault dir");
        for (relative, bytes) in files {
            let target = vault.path().join(relative);
            fs::create_dir_all(target.parent().expect("parent"))
                .expect("parent dirs");
            fs::write(&target, bytes).expect("fixture file");
        }
        vault
    }

    fn mode_of(path: &Path) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::symlink_metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            0
        }
    }

    #[test]
    fn snapshots_hash_exact_bytes_and_keep_basenames() {
        let vault = vault_with(&[
            ("files/invoice.pdf", b"%PDF bytes"),
            ("photo.png", b"\x89PNG bytes"),
        ]);
        let declared = vec![
            PathBuf::from("files/invoice.pdf"),
            PathBuf::from("photo.png"),
        ];
        let staging = AttachmentStaging::prepare(vault.path(), &declared)
            .expect("staging");
        assert_eq!(staging.files().len(), 2);
        let first = &staging.files()[0];
        assert_eq!(first.filename, "invoice.pdf");
        assert_eq!(first.len, 10);
        assert_eq!(first.snapshot.parent(), Some(staging.dir()));
        assert_ne!(first.snapshot, vault.path().join("files/invoice.pdf"));
        assert_eq!(fs::read(&first.snapshot).expect("snapshot"), b"%PDF bytes");
        let mut hash = Sha256::new();
        hash.update(b"%PDF bytes");
        assert_eq!(first.sha256, hex::encode(hash.finalize()));
        assert_eq!(staging.files()[1].filename, "photo.png");
    }

    #[test]
    fn staging_is_private_and_cleaned_up() {
        let vault = vault_with(&[("a.txt", b"bytes")]);
        let dir_path;
        let snapshot_path;
        {
            let staging = AttachmentStaging::prepare(
                vault.path(),
                &[PathBuf::from("a.txt")],
            )
            .expect("staging");
            dir_path = staging.dir().to_path_buf();
            snapshot_path = staging.files()[0].snapshot.clone();
            assert!(dir_path.is_dir());
            assert!(snapshot_path.is_file());
            #[cfg(unix)]
            {
                assert_eq!(mode_of(&dir_path), 0o700);
                assert_eq!(mode_of(&snapshot_path), 0o600);
            }
        }
        assert!(!dir_path.exists(), "staging dir is removed on drop");
        assert!(!snapshot_path.exists());
    }

    #[test]
    fn snapshots_survive_later_original_changes() {
        let vault = vault_with(&[("a.txt", b"original")]);
        let staging =
            AttachmentStaging::prepare(vault.path(), &[PathBuf::from("a.txt")])
                .expect("staging");
        fs::write(vault.path().join("a.txt"), b"mutated")
            .expect("mutate original");
        assert_eq!(
            fs::read(&staging.files()[0].snapshot).expect("snapshot"),
            b"original"
        );
    }

    #[test]
    fn duplicate_declarations_attach_once() {
        let vault = vault_with(&[("a.txt", b"bytes")]);
        let staging = AttachmentStaging::prepare(
            vault.path(),
            &[PathBuf::from("a.txt"), PathBuf::from("a.txt")],
        )
        .expect("staging");
        assert_eq!(staging.files().len(), 1);
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let vault = vault_with(&[("ok.txt", b"bytes")]);
        for raw in [
            "",
            "/absolute.txt",
            "../outside.txt",
            "sub/../../outside.txt",
            "bad\x07.txt",
        ] {
            let error =
                AttachmentStaging::prepare(vault.path(), &[PathBuf::from(raw)])
                    .expect_err("unsafe path");
            assert!(
                matches!(error, AttachmentError::UnsafePath(_)),
                "wrong error for {raw:?}: {error}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_and_non_files_are_refused() {
        use std::os::unix::fs::symlink;
        let vault = vault_with(&[
            ("real.txt", b"bytes"),
            ("sub/inner.txt", b"bytes"),
            ("empty-dir/.keep", b""),
        ]);
        symlink(vault.path().join("real.txt"), vault.path().join("link.txt"))
            .expect("file symlink");
        symlink(vault.path().join("sub"), vault.path().join("linked-dir"))
            .expect("dir symlink");
        let error = AttachmentStaging::prepare(
            vault.path(),
            &[PathBuf::from("link.txt")],
        )
        .expect_err("symlinked file");
        assert!(matches!(error, AttachmentError::UnsafePath(_)));
        let error = AttachmentStaging::prepare(
            vault.path(),
            &[PathBuf::from("linked-dir/inner.txt")],
        )
        .expect_err("symlinked traversal");
        assert!(matches!(error, AttachmentError::UnsafePath(_)));
        let error = AttachmentStaging::prepare(
            vault.path(),
            &[PathBuf::from("empty-dir")],
        )
        .expect_err("directory");
        assert!(matches!(error, AttachmentError::NotAFile(_)));
        let error = AttachmentStaging::prepare(
            vault.path(),
            &[PathBuf::from("gone.txt")],
        )
        .expect_err("missing");
        assert!(matches!(error, AttachmentError::Missing(_)));
    }

    #[test]
    fn errors_name_paths_never_bytes() {
        let vault = vault_with(&[("secret.txt", b"super secret body")]);
        let error = AttachmentStaging::prepare(
            vault.path(),
            &[PathBuf::from("gone.txt")],
        )
        .expect_err("missing");
        let message = error.to_string();
        assert!(message.contains("gone.txt"));
        assert!(!message.contains("super secret body"));
    }
}
