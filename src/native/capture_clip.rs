use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::env as bob_env;

pub(crate) const DEFAULT_HEADER: &str = "clip";
const IMAGE_EMBED_WIDTH: usize = 400;
const MAX_ATTACHMENT_COUNT: usize = 10;
const MAX_INLINE_CHARACTERS: usize = 1000;
const MAX_LINES: usize = 10;
const MAX_SLUG_CHARACTERS: usize = 40;
const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "heic", "ico", "jpeg", "jpg", "png", "svg", "tif",
    "tiff", "webp",
];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ClipMode {
    Inline,
    Lines,
    Attachments,
    Snippet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AttachmentKind {
    Image,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AttachmentOutput {
    pub(crate) source: String,
    pub(crate) saved: String,
    pub(crate) kind: AttachmentKind,
    pub(crate) reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ClipOutput {
    pub(crate) header: String,
    pub(crate) mode: ClipMode,
    pub(crate) lines: Vec<String>,
    pub(crate) attachments: Vec<AttachmentOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) snippet: Option<String>,
}

#[derive(Debug, Clone)]
struct PlannedFile {
    destination: PathBuf,
    contents: Vec<u8>,
    reused: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ClipPlan {
    pub(crate) output: ClipOutput,
    files: Vec<PlannedFile>,
}

impl ClipPlan {
    pub(crate) fn save(&self) -> Result<Vec<PathBuf>, String> {
        let mut created = Vec::new();
        let mut written = HashSet::new();

        for file in &self.files {
            if file.reused || !written.insert(file.destination.clone()) {
                continue;
            }
            if let Err(error) =
                write_new_file_atomically(&file.destination, &file.contents)
            {
                let mut message = error;
                if !created.is_empty() {
                    let cleanup = cleanup_created(&created);
                    append_cleanup_message(&mut message, &cleanup);
                }
                return Err(message);
            }
            created.push(file.destination.clone());
        }

        Ok(created)
    }
}

pub(crate) fn cleanup_created(paths: &[PathBuf]) -> Vec<String> {
    let mut failures = Vec::new();
    for path in paths.iter().rev() {
        if let Err(error) = fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            failures.push(format!("remove {}: {error}", path.display()));
        }
    }
    failures
}

pub(crate) fn append_cleanup_message(
    message: &mut String,
    failures: &[String],
) {
    if failures.is_empty() {
        message.push_str("; removed clipboard files created by this capture");
    } else {
        message.push_str("; clipboard-file cleanup also failed: ");
        message.push_str(&failures.join("; "));
    }
}

pub(crate) fn is_valid_header(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

pub(crate) fn rendered_header(value: &str) -> String {
    value.to_ascii_uppercase().replace('_', " ")
}

pub(crate) fn read_clipboard() -> Result<String, String> {
    let output = clipboard_command_output()?;
    normalize_clipboard_output(output.stdout)
}

fn clipboard_command_output() -> Result<Output, String> {
    if let Some(command) = env::var("BOB_CLIPBOARD_CMD")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let parts = command.split_whitespace().collect::<Vec<_>>();
        let (program, args) = parts.split_first().ok_or_else(|| {
            "BOB_CLIPBOARD_CMD must name a command".to_string()
        })?;
        return run_required_command(program, args, "BOB_CLIPBOARD_CMD");
    }

    #[cfg(target_os = "macos")]
    {
        return run_required_command("pbpaste", &[], "pbpaste");
    }

    #[cfg(target_os = "linux")]
    {
        if env::var_os("WAYLAND_DISPLAY").is_some() {
            return run_required_command(
                "wl-paste",
                &["--no-newline", "--type", "text"],
                "wl-paste",
            );
        }

        if env::var_os("DISPLAY").is_some() {
            match run_command("xclip", &["-selection", "clipboard", "-o"]) {
                Ok(output) => return require_success(output, "xclip"),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return run_required_command(
                        "xsel",
                        &["--clipboard", "--output"],
                        "xsel (after xclip was not found)",
                    );
                }
                Err(error) => {
                    return Err(format!("run xclip: {error}"));
                }
            }
        }
    }

    if env::var_os("TMUX").is_some() {
        return run_required_command(
            "tmux",
            &["show-buffer"],
            "tmux show-buffer",
        );
    }

    let tried = if cfg!(target_os = "macos") {
        "pbpaste and tmux"
    } else if cfg!(target_os = "linux") {
        "wl-paste, xclip/xsel, and tmux"
    } else {
        "tmux"
    };
    Err(format!(
        "no clipboard source is available (tried {tried}); set \
BOB_CLIPBOARD_CMD to a command that prints clipboard text"
    ))
}

fn run_required_command(
    program: &str,
    args: &[&str],
    label: &str,
) -> Result<Output, String> {
    let output = run_command(program, args)
        .map_err(|error| format!("run {label}: {error}"))?;
    require_success(output, label)
}

fn run_command(program: &str, args: &[&str]) -> io::Result<Output> {
    Command::new(program).args(args).output()
}

fn require_success(output: Output, label: &str) -> Result<Output, String> {
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    Err(format!(
        "clipboard command {label} exited with {}{suffix}",
        output
            .status
            .code()
            .map_or_else(|| "a signal".to_string(), |code| code.to_string())
    ))
}

fn normalize_clipboard_output(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.contains(&0) {
        return Err(
            "clipboard contains binary data (embedded NUL); copy a file path \
when attaching binary content"
                .to_string(),
        );
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        "clipboard is not valid UTF-8; copy a file path when attaching binary \
content"
            .to_string()
    })?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.split('\n').collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let normalized = lines.join("\n");
    if normalized.trim().is_empty() {
        return Err("clipboard is empty".to_string());
    }
    Ok(normalized)
}

pub(crate) fn plan(
    bob_dir: &Path,
    header: &str,
    clipboard: &str,
    now: NaiveDateTime,
) -> Result<ClipPlan, String> {
    let header = rendered_header(header);
    let lines = clipboard.split('\n').collect::<Vec<_>>();
    let path_states = lines
        .iter()
        .map(|line| classify_path_candidate(line))
        .collect::<Result<Vec<_>, _>>()?;

    for state in &path_states {
        if let PathState::Missing(path) = state {
            return Err(format!(
                "clipboard attachment does not exist: {}",
                path.display()
            ));
        }
    }

    if lines.len() == 1 {
        return match &path_states[0] {
            PathState::File(path) => {
                plan_attachments(bob_dir, &header, [path.as_path()])
            }
            _ if lines[0].chars().count() > MAX_INLINE_CHARACTERS => {
                plan_snippet(bob_dir, &header, clipboard, now)
            }
            _ => Ok(inline_plan(&header, lines[0])),
        };
    }

    let all_nonempty_attachments =
        lines.iter().zip(&path_states).all(|(line, state)| {
            !line.trim().is_empty() && matches!(state, PathState::File(_))
        });
    if all_nonempty_attachments {
        if lines.len() > MAX_ATTACHMENT_COUNT {
            return Err(format!(
                "clipboard contains {} attachments; at most {MAX_ATTACHMENT_COUNT} are supported",
                lines.len()
            ));
        }
        return plan_attachments(
            bob_dir,
            &header,
            path_states.iter().filter_map(|state| match state {
                PathState::File(path) => Some(path.as_path()),
                _ => None,
            }),
        );
    }

    if lines.len() > MAX_LINES
        || lines.iter().any(|line| line.trim().is_empty())
        || lines.iter().any(|line| is_structural_line(line))
    {
        return plan_snippet(bob_dir, &header, clipboard, now);
    }

    Ok(lines_plan(&header, &lines))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathState {
    NotPath,
    File(PathBuf),
    Directory,
    Missing(PathBuf),
}

fn classify_path_candidate(line: &str) -> Result<PathState, String> {
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

fn percent_decode(value: &str) -> Result<String, String> {
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

fn is_structural_line(line: &str) -> bool {
    if line.starts_with(' ') || line.starts_with('\t') {
        return true;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('#')
        || trimmed.starts_with("> ")
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
        || ["- ", "* ", "+ "]
            .iter()
            .any(|marker| trimmed.starts_with(marker))
    {
        return true;
    }
    let digit_count = trimmed
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digit_count > 0 && trimmed[digit_count..].starts_with(". ")
}

fn inline_plan(header: &str, text: &str) -> ClipPlan {
    ClipPlan {
        output: ClipOutput {
            header: header.to_string(),
            mode: ClipMode::Inline,
            lines: vec![format!("  - **{header}:** {text}")],
            attachments: Vec::new(),
            snippet: None,
        },
        files: Vec::new(),
    }
}

fn lines_plan(header: &str, clipboard_lines: &[&str]) -> ClipPlan {
    let mut lines = vec![format!("  - **{header}:**")];
    lines.extend(clipboard_lines.iter().map(|line| format!("    - {line}")));
    ClipPlan {
        output: ClipOutput {
            header: header.to_string(),
            mode: ClipMode::Lines,
            lines,
            attachments: Vec::new(),
            snippet: None,
        },
        files: Vec::new(),
    }
}

fn plan_attachments<'a>(
    bob_dir: &Path,
    header: &str,
    sources: impl IntoIterator<Item = &'a Path>,
) -> Result<ClipPlan, String> {
    let mut attachments = Vec::new();
    let mut files = Vec::new();
    let mut planned_hashes = HashMap::<PathBuf, String>::new();

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
            &planned_hashes,
        )?;
        planned_hashes.insert(destination.clone(), hash);
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
        files.push(PlannedFile {
            destination,
            contents,
            reused,
        });
    }

    let references = attachments
        .iter()
        .map(attachment_reference)
        .collect::<Vec<_>>();
    let lines = if references.len() == 1 {
        vec![format!("  - **{header}:** {}", references[0])]
    } else {
        let mut lines = vec![format!("  - **{header}:**")];
        lines.extend(
            references
                .iter()
                .map(|reference| format!("    - {reference}")),
        );
        lines
    };

    Ok(ClipPlan {
        output: ClipOutput {
            header: header.to_string(),
            mode: ClipMode::Attachments,
            lines,
            attachments,
            snippet: None,
        },
        files,
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
    planned: &HashMap<PathBuf, String>,
) -> Result<(PathBuf, bool), String> {
    if let Some(planned_hash) = planned.get(base) {
        if planned_hash == hash {
            return Ok((base.to_path_buf(), false));
        }
    } else if base.exists() {
        if file_matches(base, contents)? {
            return Ok((base.to_path_buf(), true));
        }
    } else {
        return Ok((base.to_path_buf(), false));
    }

    let hashed = with_hash_suffix(base, &hash[..8]);
    if let Some(planned_hash) = planned.get(&hashed) {
        if planned_hash == hash {
            return Ok((hashed, false));
        }
        return Err(format!(
            "clipboard attachment collision at {}",
            hashed.display()
        ));
    }
    if hashed.exists() {
        if file_matches(&hashed, contents)? {
            return Ok((hashed, true));
        }
        return Err(format!(
            "clipboard attachment collision at {}",
            hashed.display()
        ));
    }
    Ok((hashed, false))
}

fn file_matches(path: &Path, contents: &[u8]) -> Result<bool, String> {
    fs::read(path)
        .map(|existing| sha256_hex(&existing) == sha256_hex(contents))
        .map_err(|error| {
            format!("read existing attachment {}: {error}", path.display())
        })
}

fn sha256_hex(contents: &[u8]) -> String {
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

fn sanitize_file_name(name: &str) -> String {
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

fn plan_snippet(
    bob_dir: &Path,
    header: &str,
    clipboard: &str,
    now: NaiveDateTime,
) -> Result<ClipPlan, String> {
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
        if !destination.exists() {
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
    Ok(ClipPlan {
        output: ClipOutput {
            header: header.to_string(),
            mode: ClipMode::Snippet,
            lines: vec![format!("  - **{header}:** [[{reference}]]")],
            attachments: Vec::new(),
            snippet: Some(saved),
        },
        files: vec![PlannedFile {
            destination,
            contents,
            reused: false,
        }],
    })
}

fn snippet_slug(clipboard: &str) -> Option<String> {
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

fn write_new_file_atomically(
    destination: &Path,
    contents: &[u8],
) -> Result<(), String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("clipboard-file");
    for _ in 0..100 {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{name}.bob-capture-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "create temporary clipboard file for {}: {error}",
                    destination.display()
                ));
            }
        };
        if let Err(error) =
            file.write_all(contents).and_then(|_| file.sync_all())
        {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "write temporary clipboard file for {}: {error}",
                destination.display()
            ));
        }
        drop(file);
        if destination.exists() {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "save clipboard file {}: destination appeared after planning",
                destination.display()
            ));
        }
        match fs::rename(&temp, destination) {
            Ok(()) => {
                return Ok(());
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(format!(
                    "save clipboard file {}: {error}",
                    destination.display()
                ));
            }
        }
    }
    Err(format!(
        "could not allocate temporary clipboard file for {}",
        destination.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "bob-capture-clip-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test root");
        root
    }

    fn test_now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str(
            "2026-07-15 13:14:15",
            "%Y-%m-%d %H:%M:%S",
        )
        .expect("time")
    }

    #[test]
    fn formats_headers() {
        assert_eq!(rendered_header(DEFAULT_HEADER), "CLIP");
        assert_eq!(rendered_header("foo_bar_baz"), "FOO BAR BAZ");
        assert_eq!(rendered_header("foo-bar2"), "FOO-BAR2");
        assert!(is_valid_header("A_b-2"));
        assert!(!is_valid_header("bad!"));
    }

    #[test]
    fn normalizes_clipboard_text_and_rejects_binary_or_empty() {
        assert_eq!(
            normalize_clipboard_output(b"one\r\ntwo\rthree\n \n".to_vec())
                .expect("normalize"),
            "one\ntwo\nthree"
        );
        assert!(normalize_clipboard_output(vec![b'a', 0]).is_err());
        assert!(normalize_clipboard_output(vec![0xff]).is_err());
        assert_eq!(
            normalize_clipboard_output(b" \n\n".to_vec()).unwrap_err(),
            "clipboard is empty"
        );
    }

    #[test]
    fn detects_structural_lines() {
        for line in [
            "- item",
            "* item",
            "+ item",
            "1. item",
            "# h",
            "> q",
            "```",
            "~~~",
            "  indented",
        ] {
            assert!(is_structural_line(line), "{line}");
        }
        for line in ["plain", "1.item", ">quote"] {
            assert!(!is_structural_line(line), "{line}");
        }
    }

    #[test]
    fn sanitizes_attachment_names_and_builds_slugs() {
        assert_eq!(sanitize_file_name(" .a::b[#]文.md. "), "a-b-文.md");
        assert_eq!(sanitize_file_name("..."), "attachment");
        assert_eq!(
            snippet_slug("Hello, Wonderful World!\nnext").as_deref(),
            Some("hello-wonderful-world")
        );
    }

    #[test]
    fn renders_inline_lines_and_long_text_modes() {
        let root = Path::new("/tmp/unused-bob-clip-test");
        let now = test_now();
        let inline = plan(root, "clip", "hello", now).expect("inline");
        assert_eq!(inline.output.mode, ClipMode::Inline);
        assert_eq!(inline.output.lines, ["  - **CLIP:** hello"]);

        let lines = plan(root, "log", "one\ntwo", now).expect("lines");
        assert_eq!(lines.output.mode, ClipMode::Lines);
        assert_eq!(
            lines.output.lines,
            ["  - **LOG:**", "    - one", "    - two"]
        );

        let long = "x".repeat(MAX_INLINE_CHARACTERS + 1);
        let snippet = plan(root, "clip", &long, now).expect("snippet");
        assert_eq!(snippet.output.mode, ClipMode::Snippet);
        assert_eq!(
            snippet.output.snippet.as_deref(),
            Some("file/clip-20260715-131415-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.md")
        );
    }

    #[test]
    fn percent_decodes_file_uris() {
        assert_eq!(
            percent_decode("/tmp/hello%20world.md").expect("decode"),
            "/tmp/hello world.md"
        );
        assert!(percent_decode("/tmp/%xx").is_err());
    }

    #[test]
    fn classifies_paths_structured_text_and_attachment_limits() {
        let root = test_root("classify");
        let vault = root.join("vault");
        let source = root.join("hello world.png");
        fs::create_dir_all(&vault).expect("vault");
        fs::write(&source, b"image").expect("source");

        let uri = format!("file://{}", source.display()).replace(' ', "%20");
        let attachment = plan(&vault, "photo", &uri, test_now())
            .expect("file URI attachment");
        assert_eq!(attachment.output.mode, ClipMode::Attachments);
        assert_eq!(
            attachment.output.lines,
            ["  - **PHOTO:** ![[img/hello world.png|400]]"]
        );
        let quoted = plan(
            &vault,
            "clip",
            &format!("\"{}\"", source.display()),
            test_now(),
        )
        .expect("quoted attachment");
        assert_eq!(quoted.output.mode, ClipMode::Attachments);

        let document = root.join("document.pdf");
        fs::write(&document, b"document").expect("document source");
        let multiple = plan(
            &vault,
            "clip",
            &format!("{}\n{}", source.display(), document.display()),
            test_now(),
        )
        .expect("multiple attachments");
        assert_eq!(
            multiple.output.lines,
            [
                "  - **CLIP:**",
                "    - ![[img/hello world.png|400]]",
                "    - [[file/document.pdf]]",
            ]
        );

        let directory = plan(
            &vault,
            "clip",
            root.to_str().expect("utf8 root"),
            test_now(),
        )
        .expect("directories fall through");
        assert_eq!(directory.output.mode, ClipMode::Inline);

        for text in ["one\n\ntwo", "- one\n- two", " one\ntwo"] {
            assert_eq!(
                plan(&vault, "clip", text, test_now())
                    .expect("snippet")
                    .output
                    .mode,
                ClipMode::Snippet,
                "{text:?}"
            );
        }

        let missing = root.join("missing.txt");
        let error = plan(
            &vault,
            "clip",
            missing.to_str().expect("utf8 path"),
            test_now(),
        )
        .expect_err("missing attachment");
        assert!(error.contains("does not exist"), "{error}");

        let mut attachment_paths = Vec::new();
        for index in 0..=MAX_ATTACHMENT_COUNT {
            let path = root.join(format!("attachment-{index}.txt"));
            fs::write(&path, index.to_string()).expect("attachment source");
            attachment_paths.push(path.display().to_string());
        }
        let error =
            plan(&vault, "clip", &attachment_paths.join("\n"), test_now())
                .expect_err("too many attachments");
        assert!(error.contains("11 attachments"), "{error}");

        let plain_lines = (0..=MAX_LINES)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            plan(&vault, "clip", &plain_lines, test_now())
                .expect("long multiline snippet")
                .output
                .mode,
            ClipMode::Snippet
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn saves_reuses_and_hash_suffixes_attachments_atomically() {
        let root = test_root("save");
        let vault = root.join("vault");
        let first_dir = root.join("first");
        let second_dir = root.join("second");
        fs::create_dir_all(&vault).expect("vault");
        fs::create_dir_all(&first_dir).expect("first dir");
        fs::create_dir_all(&second_dir).expect("second dir");
        let first = first_dir.join("report:final.txt");
        let second = second_dir.join("report:final.txt");
        fs::write(&first, b"one").expect("first source");
        fs::write(&second, b"two").expect("second source");

        let initial = plan(
            &vault,
            "clip",
            first.to_str().expect("utf8 path"),
            test_now(),
        )
        .expect("initial plan");
        assert_eq!(
            initial.output.attachments[0].saved,
            "file/report-final.txt"
        );
        let created = initial.save().expect("save attachment");
        assert_eq!(created.len(), 1);

        let reused = plan(
            &vault,
            "clip",
            first.to_str().expect("utf8 path"),
            test_now(),
        )
        .expect("reuse plan");
        assert!(reused.output.attachments[0].reused);
        assert!(reused.save().expect("reuse").is_empty());

        let differing = plan(
            &vault,
            "clip",
            second.to_str().expect("utf8 path"),
            test_now(),
        )
        .expect("hash plan");
        let expected_hash = &sha256_hex(b"two")[..8];
        assert_eq!(
            differing.output.attachments[0].saved,
            format!("file/report-final-{expected_hash}.txt")
        );
        differing.save().expect("save hashed attachment");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn snippet_names_use_deterministic_collision_counters() {
        let root = test_root("snippet");
        let vault = root.join("vault");
        fs::create_dir_all(&vault).expect("vault");
        let text = "# Structured title\nbody";
        let first = plan(&vault, "clip", text, test_now()).expect("first");
        assert_eq!(
            first.output.snippet.as_deref(),
            Some("file/clip-20260715-131415-structured-title.md")
        );
        first.save().expect("save first");
        let second = plan(&vault, "clip", text, test_now()).expect("second");
        assert_eq!(
            second.output.snippet.as_deref(),
            Some("file/clip-20260715-131415-structured-title-2.md")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
