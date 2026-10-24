//! Image notes: a highlight comment line that is a local image path.
use super::*;

pub(super) const IMAGE_NOTE_MAX_SIZED_WIDTH: u32 = 600;

const SUPPORTED_IMAGE_NOTE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "svg"];
const UNSUPPORTED_IMAGE_NOTE_EXTENSIONS: &[&str] =
    &["heic", "heif", "tif", "tiff", "bmp", "avif"];

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PX_PER_METER_72DPI: f64 = 2835.0;
const RETINA_DENSITY_THRESHOLD: f64 = 1.25 * PX_PER_METER_72DPI;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImageNoteRef {
    pub(super) source_text: String,
    pub(super) extension: String,
    pub(super) supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImageNoteResolution {
    Embedded {
        vault_path: String,
        width: Option<u32>,
        write: Option<ImageAssetWrite>,
    },
    Unresolved {
        state: UnresolvedImageNoteState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImageNoteRender {
    Embedded {
        vault_path: String,
        width: Option<u32>,
    },
    Unresolved {
        source_text: String,
        state: UnresolvedImageNoteState,
    },
}

#[cfg(test)]
pub(super) fn parse_image_note_line(line: &str) -> Option<ImageNoteRef> {
    let refs = parse_image_note_refs(line);
    (refs.len() == 1).then(|| refs.into_iter().next().expect("len == 1"))
}

pub(super) fn split_image_note_lines(
    text: &str,
) -> (Vec<ImageNoteRef>, String) {
    if text.is_empty() {
        return (Vec::new(), String::new());
    }
    let mut images = Vec::new();
    let mut remaining = Vec::new();
    for line in text.lines() {
        let refs = parse_image_note_refs(line);
        if refs.is_empty() {
            remaining.push(line.to_string());
        } else {
            remaining.push(String::new());
            images.extend(refs);
        }
    }
    (images, remaining.join("\n"))
}

pub(super) fn image_note_dest_path(
    ref_note_path: &Path,
    image_note: &ImageNoteRef,
) -> Result<PathBuf> {
    let assets_dir = ref_note_assets_dir(ref_note_path)?;
    let stem = safe_image_note_stem(&image_note.source_text);
    let hash8 = image_note_hash8(&image_note.source_text);
    Ok(assets_dir.join(format!("{stem}-{hash8}.{}", image_note.extension)))
}

pub(super) fn resolve_image_note(
    config: &Config,
    ref_note_path: &Path,
    image_note: &ImageNoteRef,
    annotation_order: usize,
    block_id: &str,
) -> Result<ImageNoteResolution> {
    resolve_image_note_with_home(
        config,
        ref_note_path,
        image_note,
        annotation_order,
        block_id,
        &bob_env::home_dir(),
    )
}

pub(super) fn resolve_image_note_with_home(
    config: &Config,
    ref_note_path: &Path,
    image_note: &ImageNoteRef,
    annotation_order: usize,
    block_id: &str,
    home: &Path,
) -> Result<ImageNoteResolution> {
    if !image_note.supported {
        return Ok(ImageNoteResolution::Unresolved {
            state: UnresolvedImageNoteState::Unsupported,
        });
    }
    let dest_path = image_note_dest_path(ref_note_path, image_note)?;
    match fs::metadata(&dest_path) {
        Ok(meta) if meta.is_file() => {
            let bytes = fs::read(&dest_path).map_err(|error| {
                CommandError::new(format!(
                    "read image note destination {}: {error}",
                    dest_path.display()
                ))
            })?;
            return Ok(embedded_from_existing(config, &dest_path, &bytes));
        }
        Ok(_) => {
            return Err(CommandError::new(format!(
                "image note destination is not a file: {}",
                dest_path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CommandError::new(format!(
                "read image note destination {}: {error}",
                dest_path.display()
            )));
        }
    }

    let source_path = expand_image_note_source(&image_note.source_text, home);
    match read_image_note_source(&source_path) {
        Ok(bytes) => {
            let source_sha256 = hex::encode(Sha256::digest(&bytes));
            let width = retina_display_width(&bytes);
            let vault_relative =
                PathBuf::from(vault_relative_path_value(config, &dest_path));
            Ok(ImageNoteResolution::Embedded {
                vault_path: display_path(&vault_relative),
                width,
                write: Some(ImageAssetWrite {
                    annotation_order,
                    source_path,
                    dest_path,
                    vault_relative_dest_path: vault_relative,
                    source_sha256,
                    block_id: block_id.to_string(),
                    action: ImageAssetAction::Copy,
                }),
            })
        }
        Err(state) => Ok(ImageNoteResolution::Unresolved { state }),
    }
}

pub(super) fn expand_image_note_source(
    source_text: &str,
    home: &Path,
) -> PathBuf {
    if let Some(rest) = source_text.strip_prefix("~/") {
        home.join(rest)
    } else if source_text == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(source_text)
    }
}

pub(super) fn retina_display_width(bytes: &[u8]) -> Option<u32> {
    if !bytes.starts_with(PNG_SIGNATURE) {
        return None;
    }
    let mut cursor = PNG_SIGNATURE.len();
    let mut px_width = None;
    let mut density = None;
    while cursor + 12 <= bytes.len() {
        let length =
            u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().ok()?)
                as usize;
        cursor += 4;
        if cursor + 4 + length + 4 > bytes.len() {
            return None;
        }
        let kind = &bytes[cursor..cursor + 4];
        cursor += 4;
        let data = &bytes[cursor..cursor + length];
        cursor += length + 4;
        if kind == b"IHDR" {
            if data.len() < 4 {
                return None;
            }
            px_width = Some(u32::from_be_bytes(data[..4].try_into().ok()?));
        } else if kind == b"pHYs" {
            if data.len() < 9 {
                return None;
            }
            let ppx = u32::from_be_bytes(data[..4].try_into().ok()?);
            let unit = data[8];
            if unit == 1 {
                density = Some(ppx);
            }
        } else if kind == b"IDAT" {
            break;
        }
    }
    let px_width = px_width?;
    let density = density?;
    if density == 0 || (density as f64) <= RETINA_DENSITY_THRESHOLD {
        return None;
    }
    let logical = ((px_width as f64) * PX_PER_METER_72DPI / (density as f64))
        .round() as u32;
    (logical > 0 && logical < px_width && logical <= IMAGE_NOTE_MAX_SIZED_WIDTH)
        .then_some(logical)
}

pub(super) fn is_supported_image_note_extension(extension: &str) -> bool {
    SUPPORTED_IMAGE_NOTE_EXTENSIONS.contains(&extension)
}

pub(super) fn image_note_embed_target(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let inner = trimmed.strip_prefix("![[")?.strip_suffix("]]")?;
    if inner.is_empty() {
        return None;
    }
    let (target, hint) = match inner.split_once('|') {
        Some((target, hint)) => (target, Some(hint)),
        None => (inner, None),
    };
    if target.is_empty() {
        return None;
    }
    if let Some(hint) = hint
        && (hint.is_empty() || !hint.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let extension = Path::new(target)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)?;
    is_supported_image_note_extension(&extension).then(|| target.to_string())
}

pub(super) fn image_note_embed_markdown(
    vault_path: &str,
    width: Option<u32>,
) -> String {
    match width {
        Some(width) => format!("![[{vault_path}|{width}]]"),
        None => format!("![[{vault_path}]]"),
    }
}

pub(super) fn parse_image_note_refs(line: &str) -> Vec<ImageNoteRef> {
    let prepared = prepare_image_note_line(line);
    if prepared.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = prepared.split_whitespace().collect();
    if tokens.len() > 1 {
        let parsed: Vec<ImageNoteRef> = tokens
            .iter()
            .filter_map(|token| parse_image_note_candidate(token))
            .collect();
        if parsed.len() == tokens.len() {
            return parsed;
        }
    }
    parse_image_note_candidate(&prepared).into_iter().collect()
}

fn prepare_image_note_line(line: &str) -> String {
    let trimmed = line.trim();
    let without_list = strip_unordered_list_marker(trimmed).unwrap_or(trimmed);
    let without_label = strip_comment_label_only(without_list);
    let unwrapped = strip_one_wrapper(without_label.trim());
    strip_trailing_image_punct(unwrapped).to_string()
}

fn parse_image_note_candidate(candidate: &str) -> Option<ImageNoteRef> {
    if candidate.is_empty() {
        return None;
    }
    let unescaped = unescape_path(candidate);
    let canonical = if let Some(path) = decode_file_url(&unescaped) {
        path
    } else if unescaped.starts_with("~/") || unescaped.starts_with('/') {
        unescaped
    } else {
        return None;
    };
    if canonical.contains('\n') {
        return None;
    }
    let extension = Path::new(&canonical)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)?;
    let supported = is_supported_image_note_extension(&extension);
    let recognized = supported
        || UNSUPPORTED_IMAGE_NOTE_EXTENSIONS.contains(&extension.as_str());
    recognized.then_some(ImageNoteRef {
        source_text: canonical,
        extension,
        supported,
    })
}

fn strip_one_wrapper(text: &str) -> &str {
    const PAIRS: [(char, char); 6] = [
        ('`', '`'),
        ('<', '>'),
        ('"', '"'),
        ('\'', '\''),
        ('\u{201c}', '\u{201d}'),
        ('\u{2018}', '\u{2019}'),
    ];
    for (open, close) in PAIRS {
        if let Some(inner) = text
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
            && !inner.is_empty()
        {
            return inner;
        }
    }
    text
}

fn strip_trailing_image_punct(text: &str) -> &str {
    let Some(stripped) = text.strip_suffix(['.', ',', ';']) else {
        return text;
    };
    let extension = Path::new(stripped)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some(ext)
            if is_supported_image_note_extension(ext)
                || UNSUPPORTED_IMAGE_NOTE_EXTENSIONS.contains(&ext) =>
        {
            stripped
        }
        _ => text,
    }
}

fn unescape_path(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn decode_file_url(text: &str) -> Option<String> {
    if !text.starts_with("file://") {
        return None;
    }
    let url = url::Url::parse(text).ok()?;
    let path = url.to_file_path().ok()?;
    path.to_str().map(str::to_string)
}

fn safe_image_note_stem(source_text: &str) -> String {
    let raw = Path::new(source_text)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("");
    let mut collapsed = String::new();
    let mut in_run = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            collapsed.push(ch);
            in_run = false;
        } else if !in_run {
            collapsed.push('_');
            in_run = true;
        }
    }
    let trimmed = collapsed.trim_matches(|ch| matches!(ch, '.' | '_' | '-'));
    if trimmed.is_empty() {
        return "image".to_string();
    }
    let stem: String = trimmed.chars().take(64).collect();
    if stem.is_empty() {
        "image".to_string()
    } else {
        stem
    }
}

fn image_note_hash8(source_text: &str) -> String {
    hex::encode(Sha256::digest(source_text.as_bytes()))[..8].to_string()
}

fn embedded_from_existing(
    config: &Config,
    dest_path: &Path,
    bytes: &[u8],
) -> ImageNoteResolution {
    let vault_relative =
        PathBuf::from(vault_relative_path_value(config, dest_path));
    ImageNoteResolution::Embedded {
        vault_path: display_path(&vault_relative),
        width: retina_display_width(bytes),
        write: None,
    }
}

fn read_image_note_source(
    source_path: &Path,
) -> std::result::Result<Vec<u8>, UnresolvedImageNoteState> {
    match fs::metadata(source_path) {
        Ok(meta) if meta.is_dir() => Err(UnresolvedImageNoteState::Unreadable),
        Ok(_) => fs::read(source_path)
            .map_err(|_| UnresolvedImageNoteState::Unreadable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(UnresolvedImageNoteState::NotFound)
        }
        Err(_) => Err(UnresolvedImageNoteState::Unreadable),
    }
}
