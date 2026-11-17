use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use chrono::{Datelike, NaiveDateTime, Timelike};
use sha2::{Digest, Sha256};

use crate::native::env as bob_env;

use super::{
    model::{
        AttachmentWarning, ClipPlanError, FileReservations, PlannedFileKind,
        PlannedFileSource,
    },
    render::rendered_lines,
    AttachmentKind, AttachmentOutput, ClipMode, ClipOutput, ClipStatus,
};

pub(super) const EMBED_WIDTH: usize = 400;
pub(super) const MAX_SLUG_CHARACTERS: usize = 40;
const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "heic", "ico", "jpeg", "jpg", "png", "svg", "tif",
    "tiff", "webp",
];
pub(crate) const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "webm", "mkv"];
const STREAM_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct AttachmentLimits {
    pub(crate) warning_bytes: u64,
    pub(crate) limit_bytes: u64,
}

impl Default for AttachmentLimits {
    fn default() -> Self {
        Self {
            warning_bytes: crate::native::VAULT_SYNC_LARGE_FILE_WARNING_BYTES,
            limit_bytes: crate::native::VAULT_SYNC_LARGE_FILE_LIMIT_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PathState {
    NotPath,
    File(PathBuf),
    Directory,
    Missing(PathBuf),
}

pub(crate) fn classify_path_candidate(line: &str) -> Result<PathState, String> {
    let unwrapped = strip_matching_wrapper(line.trim());
    let decoded = if let Some(rest) = unwrapped.strip_prefix("file://") {
        let path = match rest.strip_prefix('/') {
            Some(_) => rest,
            None => rest.split_once('/').map(|(_, path)| path).unwrap_or(""),
        };
        let with_root = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        percent_decode(&with_root)?
    } else {
        unwrapped.to_string()
    };
    let expanded = bob_env::expand_tilde(Path::new(&decoded));
    if !expanded.is_absolute() {
        return Ok(PathState::NotPath);
    }
    match fs::metadata(&expanded) {
        Ok(metadata) if metadata.is_file() => Ok(PathState::File(expanded)),
        Ok(metadata) if metadata.is_dir() => Ok(PathState::Directory),
        Ok(_) => Ok(PathState::NotPath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(PathState::Missing(expanded))
        }
        Err(error) => Err(format!(
            "inspect clipboard path {}: {error}",
            expanded.display()
        )),
    }
}

fn strip_matching_wrapper(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if matches!(
            (bytes[0], bytes[value.len() - 1]),
            (b'\'', b'\'') | (b'"', b'"') | (b'<', b'>')
        ) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

pub(super) fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(high) = bytes.get(index + 1).and_then(|byte| hex(*byte))
            else {
                return Err(format!(
                    "invalid percent escape in file URI: {value}"
                ));
            };
            let Some(low) = bytes.get(index + 2).and_then(|byte| hex(*byte))
            else {
                return Err(format!(
                    "invalid percent escape in file URI: {value}"
                ));
            };
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| format!("file URI is not valid UTF-8: {value}"))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn plan_attachments<'a>(
    bob_dir: &Path,
    sources: impl IntoIterator<Item = &'a Path>,
    indent: &str,
    reservations: &mut FileReservations,
    limits: AttachmentLimits,
) -> Result<ClipOutput, ClipPlanError> {
    let mut attachments = Vec::new();
    let mut warning_rows = Vec::new();

    // Check every size before hashing any source. Besides enforcing the
    // sync ceiling, this keeps a later oversized path from making an
    // earlier large attachment do needless work.
    let mut measured = Vec::new();
    for source in sources {
        let metadata = fs::metadata(source).map_err(|error| {
            ClipPlanError::Other(format!(
                "inspect clipboard attachment {}: {error}",
                source.display()
            ))
        })?;
        let size = metadata.len();
        if size >= limits.limit_bytes {
            let display =
                crate::native::attachments::journal::display_for_source(
                    &source.display().to_string(),
                );
            let message = format!(
                "{display} is {}; vault sync refuses files of {} or more, so bob won't copy it — trim or compress it first",
                crate::native::human_bytes::human_bytes(size),
                limit_label(limits.limit_bytes),
            );
            return Err(ClipPlanError::TooLarge { message });
        }
        measured.push((source, size));
    }

    for (source, size_bytes) in measured {
        let (actual_size, hash) = hash_file(source).map_err(|error| {
            ClipPlanError::Other(format!(
                "hash clipboard attachment {}: {error}",
                source.display()
            ))
        })?;
        if actual_size != size_bytes {
            return Err(ClipPlanError::Other(format!(
                "clipboard attachment changed while planning: {}",
                source.display()
            )));
        }
        let kind = attachment_kind(source);
        let directory = match kind {
            AttachmentKind::Image => "img",
            AttachmentKind::Video => "vid",
            AttachmentKind::File => "file",
        };
        let original_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("attachment");
        let sanitized = sanitize_file_name(original_name);
        let base_destination = bob_dir.join(directory).join(&sanitized);
        let (destination, reused) = choose_attachment_destination(
            &base_destination,
            &hash,
            size_bytes,
            source,
            reservations,
        )?;
        let saved = format!(
            "{directory}/{}",
            destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&sanitized)
        );
        attachments.push(AttachmentOutput {
            source: source.display().to_string(),
            saved: saved.clone(),
            kind,
            reused,
            size_bytes,
            sha256: hash,
        });
        if size_bytes >= limits.warning_bytes {
            warning_rows.push(AttachmentWarning {
                code: "attachment_large",
                message: format!(
                    "{} is {}; vault sync warns at {} and refuses {}",
                    saved,
                    crate::native::human_bytes::human_bytes(size_bytes),
                    limit_label(limits.warning_bytes),
                    limit_label(limits.limit_bytes),
                ),
            });
        }
    }

    let destinations = attachments
        .iter()
        .map(|attachment| bob_dir.join(&attachment.saved))
        .collect::<Vec<_>>();
    check_gitignored_destinations(bob_dir, &destinations)?;
    reservations.warnings.extend(warning_rows);

    let references = attachments
        .iter()
        .map(attachment_reference)
        .collect::<Vec<_>>();
    let lines = rendered_lines(&references, indent);

    Ok(ClipOutput {
        status: ClipStatus::Read,
        count: None,
        mode: ClipMode::Attachments,
        lines,
        attachments,
        snippet: None,
        entries: Vec::new(),
        history: None,
    })
}

pub(crate) fn attachment_reference(attachment: &AttachmentOutput) -> String {
    match attachment.kind {
        AttachmentKind::Image | AttachmentKind::Video => {
            format!("![[{}|{EMBED_WIDTH}]]", attachment.saved)
        }
        AttachmentKind::File => format!("[[{}]]", attachment.saved),
    }
}

fn attachment_kind(path: &Path) -> AttachmentKind {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if extension
        .as_deref()
        .is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension))
    {
        AttachmentKind::Image
    } else if extension
        .as_deref()
        .is_some_and(|extension| VIDEO_EXTENSIONS.contains(&extension))
    {
        AttachmentKind::Video
    } else {
        AttachmentKind::File
    }
}

fn choose_attachment_destination(
    base: &Path,
    hash: &str,
    size: u64,
    source: &Path,
    reservations: &mut FileReservations,
) -> Result<(PathBuf, bool), ClipPlanError> {
    if let Some(file) = reservations.get(base) {
        if file.kind == PlannedFileKind::Attachment
            && file.size == size
            && file.sha256 == hash
        {
            let reused = file.reused;
            let is_copy_source = matches!(
                &file.source,
                PlannedFileSource::Path(path) if path == source
            );
            if reused || !is_copy_source {
                reservations.check_attachment_source(source, size, hash);
            }
            return Ok((base.to_path_buf(), reused));
        }
    } else if base.exists() {
        if file_matches(base, size, hash)? {
            reservations.reserve(
                base.to_path_buf(),
                PlannedFileSource::Path(source.to_path_buf()),
                size,
                hash.to_string(),
                true,
                PlannedFileKind::Attachment,
            );
            reservations.check_attachment_source(source, size, hash);
            return Ok((base.to_path_buf(), true));
        }
    } else {
        reservations.reserve(
            base.to_path_buf(),
            PlannedFileSource::Path(source.to_path_buf()),
            size,
            hash.to_string(),
            false,
            PlannedFileKind::Attachment,
        );
        return Ok((base.to_path_buf(), false));
    }

    let hashed = with_hash_suffix(base, &hash[..8]);
    if let Some(file) = reservations.get(&hashed) {
        if file.kind == PlannedFileKind::Attachment
            && file.size == size
            && file.sha256 == hash
        {
            let reused = file.reused;
            let is_copy_source = matches!(
                &file.source,
                PlannedFileSource::Path(path) if path == source
            );
            if reused || !is_copy_source {
                reservations.check_attachment_source(source, size, hash);
            }
            return Ok((hashed, reused));
        }
        return Err(ClipPlanError::Other(format!(
            "clipboard attachment collision at {}",
            hashed.display()
        )));
    }
    if hashed.exists() {
        if file_matches(&hashed, size, hash)? {
            reservations.reserve(
                hashed.clone(),
                PlannedFileSource::Path(source.to_path_buf()),
                size,
                hash.to_string(),
                true,
                PlannedFileKind::Attachment,
            );
            reservations.check_attachment_source(source, size, hash);
            return Ok((hashed, true));
        }
        return Err(ClipPlanError::Other(format!(
            "clipboard attachment collision at {}",
            hashed.display()
        )));
    }
    reservations.reserve(
        hashed.clone(),
        PlannedFileSource::Path(source.to_path_buf()),
        size,
        hash.to_string(),
        false,
        PlannedFileKind::Attachment,
    );
    Ok((hashed, false))
}

fn file_matches(
    path: &Path,
    size: u64,
    hash: &str,
) -> Result<bool, ClipPlanError> {
    let metadata = fs::metadata(path).map_err(|error| {
        ClipPlanError::Other(format!(
            "inspect existing attachment {}: {error}",
            path.display()
        ))
    })?;
    if metadata.len() != size {
        return Ok(false);
    }
    hash_file(path)
        .map(|(_, existing_hash)| existing_hash == hash)
        .map_err(|error| {
            ClipPlanError::Other(format!(
                "hash existing attachment {}: {error}",
                path.display()
            ))
        })
}

pub(crate) fn hash_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; STREAM_BUFFER_SIZE];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    Ok((size, hex::encode(hasher.finalize())))
}

fn limit_label(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        crate::native::human_bytes::human_bytes(bytes)
    }
}

fn check_gitignored_destinations(
    bob_dir: &Path,
    destinations: &[PathBuf],
) -> Result<(), ClipPlanError> {
    if destinations.is_empty() || !bob_dir.join(".git").exists() {
        return Ok(());
    }
    let relatives = destinations
        .iter()
        .map(|destination| {
            PathBuf::from(
                destination
                    .strip_prefix(bob_dir)
                    .unwrap_or(destination)
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        })
        .collect::<Vec<_>>();
    let matches = crate::native::vault_git::check_ignored_many(
        bob_dir,
        &git_child_env(),
        &relatives,
    )
    .unwrap_or_default();
    let ignored = matches
        .into_iter()
        .map(|matched| {
            format!(
                "{} would never leave this host: the vault .gitignore ignores it ({}) — convert it to an allowed type first",
                matched.path.display(),
                matched.rule,
            )
        })
        .collect::<Vec<_>>();
    if ignored.is_empty() {
        return Ok(());
    }
    Err(ClipPlanError::NotSynced {
        message: ignored.join("; "),
    })
}

fn git_child_env() -> crate::native::ob::ChildEnv {
    let mut child_env = crate::native::ob::child_env();
    for key in ["HOME", "GIT_CONFIG_NOSYSTEM", "PATH"] {
        if let Some(value) = bob_env::var_os(key) {
            child_env.push((OsString::from(key), value));
        }
    }
    child_env
}

pub(crate) fn sha256_hex(contents: &[u8]) -> String {
    hex::encode(Sha256::digest(contents))
}

fn with_hash_suffix(path: &Path, hash: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("attachment");
    let name = match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if !extension.is_empty() => {
            format!("{stem}-{hash}.{extension}")
        }
        _ => format!("{stem}-{hash}"),
    };
    path.with_file_name(name)
}

pub(crate) fn sanitize_file_name(name: &str) -> String {
    let mut sanitized = String::new();
    let mut replacing = false;
    for character in name.chars() {
        let forbidden = character.is_control()
            || matches!(character, '[' | ']' | '#' | '^' | '|' | ':' | '\\');
        if forbidden {
            if !replacing {
                sanitized.push('-');
            }
            replacing = true;
        } else {
            sanitized.push(character);
            replacing = false;
        }
    }
    let sanitized =
        sanitized.trim_matches(|character| matches!(character, '.' | ' '));
    if sanitized.is_empty() {
        "attachment".to_string()
    } else {
        sanitized.to_string()
    }
}

pub(super) fn plan_snippet(
    bob_dir: &Path,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut FileReservations,
) -> Result<ClipOutput, String> {
    let mut contents = clipboard.as_bytes().to_vec();
    if !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    if let Some(reserved) = reservations.files.iter().find(|file| {
        file.kind == PlannedFileKind::Snippet
            && matches!(
                &file.source,
                PlannedFileSource::Inline(existing) if existing == &contents
            )
    }) {
        let saved = reserved
            .destination
            .strip_prefix(bob_dir)
            .unwrap_or(&reserved.destination)
            .to_string_lossy()
            .replace('\\', "/");
        let reference = saved.strip_suffix(".md").unwrap_or(&saved);
        return Ok(ClipOutput {
            status: ClipStatus::Read,
            count: None,
            mode: ClipMode::Snippet,
            lines: rendered_lines(&[format!("[[{reference}]]")], indent),
            attachments: Vec::new(),
            snippet: Some(saved),
            entries: Vec::new(),
            history: None,
        });
    }
    let slug = snippet_slug(clipboard);
    let timestamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let slug_suffix = slug
        .as_deref()
        .map(|slug| format!("-{slug}"))
        .unwrap_or_default();
    let base = format!("clip-{timestamp}{slug_suffix}");
    let directory = bob_dir.join("file");
    let mut counter = 1;
    let (destination, name) = loop {
        let name = if counter == 1 {
            format!("{base}.md")
        } else {
            format!("{base}-{counter}.md")
        };
        let destination = directory.join(&name);
        if !destination.exists() && !reservations.contains(&destination) {
            break (destination, name);
        }
        counter += 1;
    };
    let saved = format!("file/{name}");
    let reference = saved.strip_suffix(".md").unwrap_or(&saved);
    let size = contents.len() as u64;
    let hash = sha256_hex(&contents);
    reservations.reserve(
        destination,
        PlannedFileSource::Inline(contents),
        size,
        hash,
        false,
        PlannedFileKind::Snippet,
    );
    Ok(ClipOutput {
        status: ClipStatus::Read,
        count: None,
        mode: ClipMode::Snippet,
        lines: rendered_lines(&[format!("[[{reference}]]")], indent),
        attachments: Vec::new(),
        snippet: Some(saved),
        entries: Vec::new(),
        history: None,
    })
}

pub(super) fn snippet_slug(clipboard: &str) -> Option<String> {
    let first = clipboard.lines().find(|line| !line.trim().is_empty())?;
    let mut slug = String::new();
    let mut separator = false;
    let mut count = 0;
    for character in first.chars() {
        if character.is_alphanumeric() {
            if separator && !slug.is_empty() && count < MAX_SLUG_CHARACTERS {
                slug.push('-');
                count += 1;
            }
            separator = false;
            for lowered in character.to_lowercase() {
                if count >= MAX_SLUG_CHARACTERS {
                    break;
                }
                slug.push(lowered);
                count += 1;
            }
        } else {
            separator = true;
        }
        if count >= MAX_SLUG_CHARACTERS {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    (!slug.is_empty()).then_some(slug)
}
