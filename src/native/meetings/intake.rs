use std::{
    error::Error as StdError,
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

pub(crate) const MAX_NOTES_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) enum NotesSource<'a> {
    Path(&'a Path),
    Stdin,
    Text(&'a str),
    /// Exact source bytes, hashed before BOM/newline normalization.
    Bytes(&'a [u8]),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotesInput {
    pub(crate) text: String,
    /// SHA-256 of the exact bytes read, before BOM and newline normalization.
    pub(crate) sha256: String,
}

#[derive(Debug)]
pub(crate) enum NotesError {
    Io {
        source: io::Error,
        path: Option<PathBuf>,
    },
    TooLarge,
    Binary,
    InvalidUtf8,
    Empty,
}

impl NotesError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Empty => "no_notes",
            Self::Io { .. }
            | Self::TooLarge
            | Self::Binary
            | Self::InvalidUtf8 => "notes_unreadable",
        }
    }
}

impl fmt::Display for NotesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                source,
                path: Some(path),
            } => {
                write!(
                    f,
                    "could not read notes from {}: {source}",
                    path.display()
                )
            }
            Self::Io { source, path: None } => {
                write!(f, "could not read notes: {source}")
            }
            Self::TooLarge => write!(f, "meeting notes exceed the 1 MiB limit"),
            Self::Binary => write!(f, "meeting notes contain binary data"),
            Self::InvalidUtf8 => write!(f, "meeting notes are not valid UTF-8"),
            Self::Empty => write!(f, "meeting notes are empty"),
        }
    }
}

impl StdError for NotesError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn read_notes(
    source: NotesSource<'_>,
) -> Result<NotesInput, NotesError> {
    let bytes = match source {
        NotesSource::Text(text) => text.as_bytes().to_vec(),
        NotesSource::Bytes(bytes) => bytes.to_vec(),
        NotesSource::Path(path) => read_path(path)?,
        NotesSource::Stdin => read_bounded(io::stdin().lock())
            .map_err(|source| NotesError::Io { source, path: None })?,
    };

    if bytes.len() > MAX_NOTES_BYTES {
        return Err(NotesError::TooLarge);
    }
    if bytes.contains(&0) {
        return Err(NotesError::Binary);
    }

    let sha256 = hex::encode(Sha256::digest(&bytes));
    let decoded =
        String::from_utf8(bytes).map_err(|_| NotesError::InvalidUtf8)?;
    let without_bom = decoded.strip_prefix('\u{feff}').unwrap_or(&decoded);
    let normalized = without_bom
        .replace("\r\n", "\n")
        .lines()
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n");
    if normalized.trim().is_empty() {
        return Err(NotesError::Empty);
    }

    Ok(NotesInput {
        text: normalized,
        sha256,
    })
}

fn read_path(path: &Path) -> Result<Vec<u8>, NotesError> {
    let file = fs::File::open(path).map_err(|source| NotesError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    read_bounded(file).map_err(|source| NotesError::Io {
        source,
        path: Some(path.to_path_buf()),
    })
}

fn read_bounded<R: Read>(reader: R) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_NOTES_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}
