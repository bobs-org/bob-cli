use std::{
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{Datelike, NaiveDateTime, Timelike};
use sha2::{Digest, Sha256};

use crate::native::env as bob_env;

use super::{
    model::{FileReservations, PlannedFileKind},
    render::rendered_lines,
    AttachmentKind, AttachmentOutput, ClipMode, ClipOutput,
};

pub(super) const IMAGE_EMBED_WIDTH: usize = 400;
pub(super) const MAX_SLUG_CHARACTERS: usize = 40;
const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "heic", "ico", "jpeg", "jpg", "png", "svg", "tif",
    "tiff", "webp",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PathState {
    NotPath,
    File(PathBuf),
    Directory,
    Missing(PathBuf),
}

pub(super) fn classify_path_candidate(line: &str) -> Result<PathState, String> {
    let unquoted = strip_matching_quotes(line.trim());
    let decoded = if let Some(rest) = unquoted.strip_prefix("file://") {
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
        unquoted.to_string()
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

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if matches!(
            (bytes[0], bytes[value.len() - 1]),
            (b'\'', b'\'') | (b'"', b'"')
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
    header: Option<&str>,
    sources: impl IntoIterator<Item = &'a Path>,
    indent: &str,
    reservations: &mut FileReservations,
) -> Result<ClipOutput, String> {
    let mut attachments = Vec::new();

    for source in sources {
        let contents = fs::read(source).map_err(|error| {
            format!("read clipboard attachment {}: {error}", source.display())
        })?;
        let hash = sha256_hex(&contents);
        let kind = attachment_kind(source);
        let directory = match kind {
            AttachmentKind::Image => "img",
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
            &contents,
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
            saved,
            kind,
            reused,
        });
    }

    let references = attachments
        .iter()
        .map(attachment_reference)
        .collect::<Vec<_>>();
    let lines = rendered_lines(header, &references, indent);

    Ok(ClipOutput {
        header: header.map(str::to_string),
        mode: ClipMode::Attachments,
        lines,
        attachments,
        snippet: None,
        entries: Vec::new(),
    })
}

fn attachment_reference(attachment: &AttachmentOutput) -> String {
    match attachment.kind {
        AttachmentKind::Image => {
            format!("![[{}|{IMAGE_EMBED_WIDTH}]]", attachment.saved)
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
    } else {
        AttachmentKind::File
    }
}

fn choose_attachment_destination(
    base: &Path,
    hash: &str,
    contents: &[u8],
    reservations: &mut FileReservations,
) -> Result<(PathBuf, bool), String> {
    if let Some(file) = reservations.get(base) {
        if file.kind == PlannedFileKind::Attachment
            && sha256_hex(&file.contents) == hash
        {
            return Ok((base.to_path_buf(), file.reused));
        }
    } else if base.exists() {
        if file_matches(base, contents)? {
            reservations.reserve(
                base.to_path_buf(),
                contents.to_vec(),
                true,
                PlannedFileKind::Attachment,
            );
            return Ok((base.to_path_buf(), true));
        }
    } else {
        reservations.reserve(
            base.to_path_buf(),
            contents.to_vec(),
            false,
            PlannedFileKind::Attachment,
        );
        return Ok((base.to_path_buf(), false));
    }

    let hashed = with_hash_suffix(base, &hash[..8]);
    if let Some(file) = reservations.get(&hashed) {
        if file.kind == PlannedFileKind::Attachment
            && sha256_hex(&file.contents) == hash
        {
            return Ok((hashed, file.reused));
        }
        return Err(format!(
            "clipboard attachment collision at {}",
            hashed.display()
        ));
    }
    if hashed.exists() {
        if file_matches(&hashed, contents)? {
            reservations.reserve(
                hashed.clone(),
                contents.to_vec(),
                true,
                PlannedFileKind::Attachment,
            );
            return Ok((hashed, true));
        }
        return Err(format!(
            "clipboard attachment collision at {}",
            hashed.display()
        ));
    }
    reservations.reserve(
        hashed.clone(),
        contents.to_vec(),
        false,
        PlannedFileKind::Attachment,
    );
    Ok((hashed, false))
}

fn file_matches(path: &Path, contents: &[u8]) -> Result<bool, String> {
    fs::read(path)
        .map(|existing| sha256_hex(&existing) == sha256_hex(contents))
        .map_err(|error| {
            format!("read existing attachment {}: {error}", path.display())
        })
}

pub(super) fn sha256_hex(contents: &[u8]) -> String {
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

pub(super) fn sanitize_file_name(name: &str) -> String {
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
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
    indent: &str,
    reservations: &mut FileReservations,
) -> Result<ClipOutput, String> {
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
    let mut contents = clipboard.as_bytes().to_vec();
    if !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    reservations.reserve(
        destination,
        contents,
        false,
        PlannedFileKind::Snippet,
    );
    Ok(ClipOutput {
        header: header.map(str::to_string),
        mode: ClipMode::Snippet,
        lines: rendered_lines(header, &[format!("[[{reference}]]")], indent),
        attachments: Vec::new(),
        snippet: Some(saved),
        entries: Vec::new(),
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
