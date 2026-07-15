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

#[cfg(any(target_os = "macos", test))]
use rusqlite::{Connection, OpenFlags};
#[cfg(any(target_os = "macos", test))]
use std::io::Cursor;

use super::env as bob_env;

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
    History,
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
    pub(crate) header: Option<String>,
    pub(crate) mode: ClipMode,
    pub(crate) lines: Vec<String>,
    pub(crate) attachments: Vec<AttachmentOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) snippet: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) entries: Vec<ClipOutput>,
}

impl ClipOutput {
    pub(crate) fn file_confirmations(&self) -> Vec<(String, bool)> {
        let outputs = if self.entries.is_empty() {
            std::slice::from_ref(self)
        } else {
            self.entries.as_slice()
        };
        let mut seen = HashSet::new();
        let mut confirmations = Vec::new();
        for output in outputs {
            for attachment in &output.attachments {
                if seen.insert(attachment.saved.clone()) {
                    confirmations
                        .push((attachment.saved.clone(), attachment.reused));
                }
            }
            if let Some(snippet) = &output.snippet
                && seen.insert(snippet.clone())
            {
                confirmations.push((snippet.clone(), false));
            }
        }
        confirmations
    }
}

#[derive(Debug, Clone)]
struct PlannedFile {
    destination: PathBuf,
    contents: Vec<u8>,
    reused: bool,
    kind: PlannedFileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedFileKind {
    Attachment,
    Snippet,
}

#[derive(Debug, Clone)]
pub(crate) struct ClipPlan {
    pub(crate) output: ClipOutput,
    files: Vec<PlannedFile>,
}

#[derive(Debug, Default)]
struct FileReservations {
    files: Vec<PlannedFile>,
    by_destination: HashMap<PathBuf, usize>,
}

impl FileReservations {
    fn contains(&self, destination: &Path) -> bool {
        self.by_destination.contains_key(destination)
    }

    fn get(&self, destination: &Path) -> Option<&PlannedFile> {
        self.by_destination
            .get(destination)
            .map(|index| &self.files[*index])
    }

    fn reserve(
        &mut self,
        destination: PathBuf,
        contents: Vec<u8>,
        reused: bool,
        kind: PlannedFileKind,
    ) {
        let index = self.files.len();
        self.by_destination.insert(destination.clone(), index);
        self.files.push(PlannedFile {
            destination,
            contents,
            reused,
            kind,
        });
    }
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

pub(crate) fn read_clipboard_history(
    count: usize,
) -> Result<Vec<String>, String> {
    let current = read_clipboard()?;
    if count == 1 {
        return Ok(vec![current]);
    }

    let candidates = read_history_candidates(count)?
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            normalize_clipboard_output(value.into_bytes()).map_err(|error| {
                format!("clipboard history entry {}: {error}", index + 1)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    merge_history_candidates(current, candidates, count)
}

fn merge_history_candidates(
    current: String,
    mut candidates: Vec<String>,
    count: usize,
) -> Result<Vec<String>, String> {
    if let Some(index) = candidates.iter().position(|value| value == &current) {
        candidates.remove(index);
    }

    let available = candidates.len() + 1;
    if available < count {
        return Err(format!(
            "clipboard history requested {count} entries but only {available} are available"
        ));
    }

    let mut values = Vec::with_capacity(count);
    values.push(current);
    values.extend(candidates.into_iter().take(count - 1));
    Ok(values)
}

fn read_history_candidates(count: usize) -> Result<Vec<String>, String> {
    if let Some(command) = env::var("BOB_CLIPBOARD_HISTORY_CMD")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return read_history_command(&command, count);
    }

    #[cfg(target_os = "macos")]
    {
        let database = bob_env::home_dir()
            .join("Library/Application Support/com.clipy-app.Clipy/sqlite.db");
        return read_clipy_history(&database, count);
    }

    #[cfg(not(target_os = "macos"))]
    Err("clipboard history is unavailable on this platform; set \
BOB_CLIPBOARD_HISTORY_CMD to a command that accepts the requested count and \
prints a newest-first JSON array of clipboard strings"
        .to_string())
}

fn read_history_command(
    command: &str,
    count: usize,
) -> Result<Vec<String>, String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let (program, args) = parts.split_first().ok_or_else(|| {
        "BOB_CLIPBOARD_HISTORY_CMD must name a command".to_string()
    })?;
    let output = Command::new(program)
        .args(args)
        .arg(count.to_string())
        .output()
        .map_err(|error| format!("run BOB_CLIPBOARD_HISTORY_CMD: {error}"))?;
    let output = require_success(output, "BOB_CLIPBOARD_HISTORY_CMD")?;
    serde_json::from_slice::<Vec<String>>(&output.stdout).map_err(|error| {
        format!(
            "BOB_CLIPBOARD_HISTORY_CMD must print a UTF-8 JSON array of strings ordered newest first: {error}"
        )
    })
}

#[cfg(any(target_os = "macos", test))]
fn read_clipy_history(
    database: &Path,
    count: usize,
) -> Result<Vec<String>, String> {
    if !database.is_file() {
        return Err(format!(
            "Clipy clipboard history database was not found at {}; install and run Clipy or set BOB_CLIPBOARD_HISTORY_CMD",
            database.display()
        ));
    }
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        format!(
            "open Clipy clipboard history database {} read-only: {error}",
            database.display()
        )
    })?;
    validate_clipy_schema(&connection)?;

    let mut statement = connection
        .prepare(
            "SELECT id FROM pasteboardHistories \
             ORDER BY updateAt DESC, id DESC LIMIT ?1",
        )
        .map_err(|error| format!("query Clipy clipboard history: {error}"))?;
    let ids = statement
        .query_map([i64::try_from(count).unwrap_or(i64::MAX)], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("query Clipy clipboard history: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!("decode Clipy clipboard history rows: {error}")
        })?;

    ids.iter()
        .enumerate()
        .map(|(index, id)| decode_clipy_entry(&connection, id, index + 1))
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn validate_clipy_schema(connection: &Connection) -> Result<(), String> {
    validate_clipy_table(
        connection,
        "pasteboardHistories",
        &[("id", "TEXT"), ("updateAt", "INTEGER")],
    )?;
    validate_clipy_table(
        connection,
        "pasteboardHistoryAssets",
        &[
            ("id", "TEXT"),
            ("pasteboardHistoryID", "TEXT"),
            ("index", "INTEGER"),
            ("pasteboardType", "TEXT"),
            ("data", "BLOB"),
        ],
    )
}

#[cfg(any(target_os = "macos", test))]
fn validate_clipy_table(
    connection: &Connection,
    table: &str,
    required: &[(&str, &str)],
) -> Result<(), String> {
    let sql = format!("PRAGMA table_info(\"{table}\")");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("inspect Clipy table {table}: {error}"))?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?.to_ascii_uppercase(),
            ))
        })
        .map_err(|error| format!("inspect Clipy table {table}: {error}"))?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| format!("inspect Clipy table {table}: {error}"))?;
    let missing = required
        .iter()
        .filter(|(column, _)| !columns.contains_key(*column))
        .map(|(column, _)| *column)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "unsupported or unmigrated Clipy database: table {table} is missing required column(s): {}",
            missing.join(", ")
        ));
    }
    let wrong_types = required
        .iter()
        .filter_map(|(column, expected)| {
            let actual = columns.get(*column)?;
            (actual != expected)
                .then(|| format!("{column} is {actual}, expected {expected}"))
        })
        .collect::<Vec<_>>();
    if !wrong_types.is_empty() {
        return Err(format!(
            "unsupported or unmigrated Clipy database: table {table} has incompatible column type(s): {}",
            wrong_types.join(", ")
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn decode_clipy_entry(
    connection: &Connection,
    history_id: &str,
    entry_index: usize,
) -> Result<String, String> {
    let mut statement = connection
        .prepare(
            "SELECT pasteboardType, data FROM pasteboardHistoryAssets \
             WHERE pasteboardHistoryID = ?1 ORDER BY \"index\" ASC, id ASC",
        )
        .map_err(|error| {
            format!("query Clipy history entry {entry_index}: {error}")
        })?;
    let assets = statement
        .query_map([history_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| {
            format!("query Clipy history entry {entry_index}: {error}")
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!("decode Clipy history entry {entry_index} assets: {error}")
        })?;

    let mut values = Vec::new();
    let mut unsupported = Vec::new();
    for (pasteboard_type, data) in assets {
        match decode_clipy_asset(&pasteboard_type, &data).map_err(|error| {
            format!(
                "Clipy history entry {entry_index} has invalid {pasteboard_type} data: {error}"
            )
        })? {
            Some(mut decoded) => values.append(&mut decoded),
            None => unsupported.push(pasteboard_type),
        }
    }
    if values.is_empty() {
        unsupported.sort();
        unsupported.dedup();
        let types = if unsupported.is_empty() {
            "no stored assets".to_string()
        } else {
            unsupported.join(", ")
        };
        return Err(format!(
            "Clipy history entry {entry_index} has only unsupported binary representations ({types}); copy text or file/URL content instead"
        ));
    }
    Ok(values.join("\n"))
}

#[cfg(any(target_os = "macos", test))]
fn decode_clipy_asset(
    pasteboard_type: &str,
    data: &[u8],
) -> Result<Option<Vec<String>>, String> {
    match pasteboard_type {
        "public.utf8-plain-text"
        | "NSStringPboardType"
        | "public.url"
        | "NSURLPboardType"
        | "public.file-url" => {
            decode_utf8_asset(data).map(|value| Some(vec![value]))
        }
        "NSFilenamesPboardType" => decode_filenames_asset(data).map(Some),
        _ => Ok(None),
    }
}

#[cfg(any(target_os = "macos", test))]
fn decode_utf8_asset(data: &[u8]) -> Result<String, String> {
    if let Ok(value) = String::from_utf8(data.to_vec()) {
        return Ok(value);
    }
    plist::Value::from_reader(Cursor::new(data))
        .ok()
        .and_then(plist::Value::into_string)
        .ok_or_else(|| "value is not valid UTF-8 text".to_string())
}

#[cfg(any(target_os = "macos", test))]
fn decode_filenames_asset(data: &[u8]) -> Result<Vec<String>, String> {
    let value = plist::Value::from_reader(Cursor::new(data))
        .map_err(|error| format!("invalid filenames property list: {error}"))?;
    let filenames = value
        .into_array()
        .ok_or_else(|| "filenames property list is not an array".to_string())?;
    filenames
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            value.into_string().ok_or_else(|| {
                format!("filename {} is not a string", index + 1)
            })
        })
        .collect()
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
        run_required_command("pbpaste", &[], "pbpaste")
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

    #[cfg(not(target_os = "macos"))]
    if env::var_os("TMUX").is_some() {
        return run_required_command(
            "tmux",
            &["show-buffer"],
            "tmux show-buffer",
        );
    }

    #[cfg(not(target_os = "macos"))]
    let tried = if cfg!(target_os = "macos") {
        "pbpaste and tmux"
    } else if cfg!(target_os = "linux") {
        "wl-paste, xclip/xsel, and tmux"
    } else {
        "tmux"
    };
    #[cfg(not(target_os = "macos"))]
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
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
) -> Result<ClipPlan, String> {
    let mut reservations = FileReservations::default();
    let output =
        plan_entry(bob_dir, header, clipboard, now, &mut reservations)?;
    Ok(ClipPlan {
        output,
        files: reservations.files,
    })
}

pub(crate) fn plan_history(
    bob_dir: &Path,
    clipboards: &[String],
    now: NaiveDateTime,
) -> Result<ClipPlan, String> {
    let mut reservations = FileReservations::default();
    let entries = clipboards
        .iter()
        .enumerate()
        .map(|(index, clipboard)| {
            plan_entry(bob_dir, None, clipboard, now, &mut reservations)
                .map_err(|error| {
                    format!("clipboard history entry {}: {error}", index + 1)
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let lines = entries
        .iter()
        .flat_map(|entry| entry.lines.iter().cloned())
        .collect();
    let attachments = entries
        .iter()
        .flat_map(|entry| entry.attachments.iter().cloned())
        .collect();
    Ok(ClipPlan {
        output: ClipOutput {
            header: None,
            mode: ClipMode::History,
            lines,
            attachments,
            snippet: None,
            entries,
        },
        files: reservations.files,
    })
}

fn plan_entry(
    bob_dir: &Path,
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
    reservations: &mut FileReservations,
) -> Result<ClipOutput, String> {
    let header = header.map(rendered_header);
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
            PathState::File(path) => plan_attachments(
                bob_dir,
                header.as_deref(),
                [path.as_path()],
                reservations,
            ),
            _ if lines[0].chars().count() > MAX_INLINE_CHARACTERS => {
                plan_snippet(
                    bob_dir,
                    header.as_deref(),
                    clipboard,
                    now,
                    reservations,
                )
            }
            _ => Ok(inline_output(header.as_deref(), lines[0])),
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
            header.as_deref(),
            path_states.iter().filter_map(|state| match state {
                PathState::File(path) => Some(path.as_path()),
                _ => None,
            }),
            reservations,
        );
    }

    if lines.len() > MAX_LINES
        || lines.iter().any(|line| line.trim().is_empty())
        || lines.iter().any(|line| is_structural_line(line))
    {
        return plan_snippet(
            bob_dir,
            header.as_deref(),
            clipboard,
            now,
            reservations,
        );
    }

    Ok(lines_output(header.as_deref(), &lines))
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

fn rendered_lines(header: Option<&str>, items: &[String]) -> Vec<String> {
    if let Some(header) = header {
        if items.len() == 1 {
            return vec![format!("  - **{header}:** {}", items[0])];
        }
        let mut lines = vec![format!("  - **{header}:**")];
        lines.extend(items.iter().map(|item| format!("    - {item}")));
        return lines;
    }

    items.iter().map(|item| format!("  - {item}")).collect()
}

fn inline_output(header: Option<&str>, text: &str) -> ClipOutput {
    ClipOutput {
        header: header.map(str::to_string),
        mode: ClipMode::Inline,
        lines: rendered_lines(header, &[text.to_string()]),
        attachments: Vec::new(),
        snippet: None,
        entries: Vec::new(),
    }
}

fn lines_output(header: Option<&str>, clipboard_lines: &[&str]) -> ClipOutput {
    let items = clipboard_lines
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    ClipOutput {
        header: header.map(str::to_string),
        mode: ClipMode::Lines,
        lines: rendered_lines(header, &items),
        attachments: Vec::new(),
        snippet: None,
        entries: Vec::new(),
    }
}

fn plan_attachments<'a>(
    bob_dir: &Path,
    header: Option<&str>,
    sources: impl IntoIterator<Item = &'a Path>,
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
    let lines = rendered_lines(header, &references);

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
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
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
        lines: rendered_lines(header, &[format!("[[{reference}]]")]),
        attachments: Vec::new(),
        snippet: Some(saved),
        entries: Vec::new(),
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
        assert_eq!(rendered_header("clip"), "CLIP");
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
    fn merges_live_clipboard_with_up_to_date_and_lagging_histories() {
        assert_eq!(
            merge_history_candidates(
                "current".to_string(),
                vec![
                    "current".to_string(),
                    "older".to_string(),
                    "oldest".to_string(),
                ],
                3,
            )
            .expect("up-to-date history"),
            ["current", "older", "oldest"]
        );
        assert_eq!(
            merge_history_candidates(
                "current".to_string(),
                vec![
                    "older".to_string(),
                    "current".to_string(),
                    "oldest".to_string(),
                ],
                3,
            )
            .expect("lagging history"),
            ["current", "older", "oldest"]
        );
        assert_eq!(
            merge_history_candidates(
                "same".to_string(),
                vec![
                    "same".to_string(),
                    "same".to_string(),
                    "older".to_string(),
                ],
                3,
            )
            .expect("later duplicate"),
            ["same", "same", "older"]
        );
        let error = merge_history_candidates(
            "current".to_string(),
            vec!["older".to_string()],
            3,
        )
        .expect_err("insufficient history");
        assert!(error.contains("requested 3") && error.contains("only 2"));
    }

    #[test]
    fn reads_clipy_sqlite_assets_in_deterministic_order() {
        let root = test_root("clipy-database");
        let database = root.join("sqlite.db");
        let connection = Connection::open(&database).expect("fixture database");
        create_clipy_fixture_schema(&connection);
        connection
            .execute(
                "INSERT INTO pasteboardHistories (id, updateAt) VALUES (?1, ?2)",
                ("older", 1_i64),
            )
            .expect("older history");
        connection
            .execute(
                "INSERT INTO pasteboardHistories (id, updateAt) VALUES (?1, ?2)",
                ("same-a", 2_i64),
            )
            .expect("tie history a");
        connection
            .execute(
                "INSERT INTO pasteboardHistories (id, updateAt) VALUES (?1, ?2)",
                ("same-b", 2_i64),
            )
            .expect("tie history b");
        insert_clipy_asset(
            &connection,
            "asset-1",
            "same-b",
            0,
            "public.utf8-plain-text",
            b"newest\r\ntext",
        );
        insert_clipy_asset(
            &connection,
            "asset-2",
            "same-a",
            0,
            "public.file-url",
            b"file:///tmp/first.txt",
        );
        insert_clipy_asset(
            &connection,
            "asset-3",
            "same-a",
            1,
            "public.file-url",
            b"file:///tmp/second.txt",
        );
        let mut filenames = Vec::new();
        plist::Value::Array(vec![
            plist::Value::String("/tmp/legacy one.txt".to_string()),
            plist::Value::String("/tmp/legacy two.txt".to_string()),
        ])
        .to_writer_xml(&mut filenames)
        .expect("filenames plist");
        insert_clipy_asset(
            &connection,
            "asset-4",
            "older",
            0,
            "NSFilenamesPboardType",
            &filenames,
        );
        drop(connection);

        let history =
            read_clipy_history(&database, 4).expect("read Clipy fixture");
        assert_eq!(
            history,
            [
                "newest\r\ntext",
                "file:///tmp/first.txt\nfile:///tmp/second.txt",
                "/tmp/legacy one.txt\n/tmp/legacy two.txt",
            ]
        );
        let normalized = history
            .into_iter()
            .map(|value| normalize_clipboard_output(value.into_bytes()))
            .collect::<Result<Vec<_>, _>>()
            .expect("normalize fixture history");
        let error =
            merge_history_candidates("newest\ntext".to_string(), normalized, 4)
                .expect_err("fixture is insufficient for four entries");
        assert!(error.contains("requested 4") && error.contains("only 3"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_unsupported_and_unmigrated_clipy_databases() {
        let root = test_root("clipy-errors");
        let unsupported = root.join("unsupported.db");
        let connection = Connection::open(&unsupported).expect("fixture");
        create_clipy_fixture_schema(&connection);
        connection
            .execute(
                "INSERT INTO pasteboardHistories (id, updateAt) VALUES ('image', 1)",
                [],
            )
            .expect("history");
        insert_clipy_asset(
            &connection,
            "asset",
            "image",
            0,
            "public.png",
            b"binary",
        );
        drop(connection);
        let error = read_clipy_history(&unsupported, 1)
            .expect_err("binary-only entry must fail");
        assert!(
            error.contains("entry 1") && error.contains("public.png"),
            "{error}"
        );

        let unmigrated = root.join("unmigrated.db");
        let connection = Connection::open(&unmigrated).expect("fixture");
        connection
            .execute("CREATE TABLE pasteboardHistories (id TEXT)", [])
            .expect("partial schema");
        drop(connection);
        let error = read_clipy_history(&unmigrated, 1)
            .expect_err("unmigrated database must fail");
        assert!(
            error.contains("unsupported or unmigrated")
                && error.contains("updateAt"),
            "{error}"
        );
        fs::remove_dir_all(root).expect("cleanup");
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
        let headerless_inline =
            plan(root, None, "hello", now).expect("headerless inline");
        assert_eq!(headerless_inline.output.header, None);
        assert_eq!(headerless_inline.output.mode, ClipMode::Inline);
        assert_eq!(headerless_inline.output.lines, ["  - hello"]);
        let single_json = serde_json::to_value(&headerless_inline.output)
            .expect("single output JSON");
        assert!(single_json.get("entries").is_none(), "{single_json}");

        let inline =
            plan(root, Some("clip"), "hello", now).expect("headed inline");
        assert_eq!(inline.output.header.as_deref(), Some("CLIP"));
        assert_eq!(inline.output.mode, ClipMode::Inline);
        assert_eq!(inline.output.lines, ["  - **CLIP:** hello"]);

        let headerless_lines =
            plan(root, None, "one\ntwo", now).expect("headerless lines");
        assert_eq!(headerless_lines.output.header, None);
        assert_eq!(headerless_lines.output.mode, ClipMode::Lines);
        assert_eq!(headerless_lines.output.lines, ["  - one", "  - two"]);

        let lines =
            plan(root, Some("log"), "one\ntwo", now).expect("headed lines");
        assert_eq!(lines.output.mode, ClipMode::Lines);
        assert_eq!(
            lines.output.lines,
            ["  - **LOG:**", "    - one", "    - two"]
        );

        let long = "x".repeat(MAX_INLINE_CHARACTERS + 1);
        let headerless_snippet =
            plan(root, None, &long, now).expect("headerless snippet");
        assert_eq!(headerless_snippet.output.header, None);
        assert_eq!(headerless_snippet.output.mode, ClipMode::Snippet);
        assert_eq!(
            headerless_snippet.output.lines,
            ["  - [[file/clip-20260715-131415-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]]"]
        );

        let snippet =
            plan(root, Some("clip"), &long, now).expect("headed snippet");
        assert_eq!(snippet.output.mode, ClipMode::Snippet);
        assert_eq!(
            snippet.output.lines,
            ["  - **CLIP:** [[file/clip-20260715-131415-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]]"]
        );
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
        let attachment = plan(&vault, Some("photo"), &uri, test_now())
            .expect("file URI attachment");
        assert_eq!(attachment.output.mode, ClipMode::Attachments);
        assert_eq!(
            attachment.output.lines,
            ["  - **PHOTO:** ![[img/hello world.png|400]]"]
        );
        let quoted = plan(
            &vault,
            Some("clip"),
            &format!("\"{}\"", source.display()),
            test_now(),
        )
        .expect("quoted attachment");
        assert_eq!(quoted.output.mode, ClipMode::Attachments);

        let document = root.join("document.pdf");
        fs::write(&document, b"document").expect("document source");
        let multiple = plan(
            &vault,
            Some("clip"),
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

        let headerless_attachment = plan(&vault, None, &uri, test_now())
            .expect("headerless attachment");
        assert_eq!(headerless_attachment.output.header, None);
        assert_eq!(
            headerless_attachment.output.lines,
            ["  - ![[img/hello world.png|400]]"]
        );
        let headerless_multiple = plan(
            &vault,
            None,
            &format!("{}\n{}", source.display(), document.display()),
            test_now(),
        )
        .expect("headerless multiple attachments");
        assert_eq!(
            headerless_multiple.output.lines,
            [
                "  - ![[img/hello world.png|400]]",
                "  - [[file/document.pdf]]",
            ]
        );

        let directory = plan(
            &vault,
            Some("clip"),
            root.to_str().expect("utf8 root"),
            test_now(),
        )
        .expect("directories fall through");
        assert_eq!(directory.output.mode, ClipMode::Inline);

        for text in ["one\n\ntwo", "- one\n- two", " one\ntwo"] {
            assert_eq!(
                plan(&vault, Some("clip"), text, test_now())
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
            Some("clip"),
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
        let error = plan(
            &vault,
            Some("clip"),
            &attachment_paths.join("\n"),
            test_now(),
        )
        .expect_err("too many attachments");
        assert!(error.contains("11 attachments"), "{error}");

        let plain_lines = (0..=MAX_LINES)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            plan(&vault, Some("clip"), &plain_lines, test_now())
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
            Some("clip"),
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
            Some("clip"),
            first.to_str().expect("utf8 path"),
            test_now(),
        )
        .expect("reuse plan");
        assert!(reused.output.attachments[0].reused);
        assert!(reused.save().expect("reuse").is_empty());

        let differing = plan(
            &vault,
            Some("clip"),
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
        let first =
            plan(&vault, Some("clip"), text, test_now()).expect("first");
        assert_eq!(
            first.output.snippet.as_deref(),
            Some("file/clip-20260715-131415-structured-title.md")
        );
        first.save().expect("save first");
        let second =
            plan(&vault, Some("clip"), text, test_now()).expect("second");
        assert_eq!(
            second.output.snippet.as_deref(),
            Some("file/clip-20260715-131415-structured-title-2.md")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn aggregate_planner_flattens_entries_and_reserves_all_paths() {
        let root = test_root("aggregate");
        let vault = root.join("vault");
        let first_dir = root.join("first");
        let second_dir = root.join("second");
        fs::create_dir_all(&vault).expect("vault");
        fs::create_dir_all(&first_dir).expect("first dir");
        fs::create_dir_all(&second_dir).expect("second dir");
        let first = first_dir.join("report.txt");
        let second = second_dir.join("report.txt");
        fs::write(&first, b"same").expect("first source");
        fs::write(&second, b"different").expect("second source");
        let snippet = "# Heading\n\nbody".to_string();
        let clipboards = vec![
            "inline".to_string(),
            "line one\nline two".to_string(),
            first.display().to_string(),
            second.display().to_string(),
            first.display().to_string(),
            snippet.clone(),
            snippet,
        ];
        let plan =
            plan_history(&vault, &clipboards, test_now()).expect("aggregate");

        assert_eq!(plan.output.mode, ClipMode::History);
        assert_eq!(plan.output.header, None);
        assert_eq!(plan.output.entries.len(), clipboards.len());
        assert_eq!(plan.output.lines[0], "  - inline");
        assert_eq!(&plan.output.lines[1..3], ["  - line one", "  - line two"]);
        assert!(plan
            .output
            .lines
            .iter()
            .all(|line| !line.contains("**CLIP:**")));
        let expected_hash = &sha256_hex(b"different")[..8];
        assert_eq!(
            plan.output
                .attachments
                .iter()
                .map(|attachment| attachment.saved.as_str())
                .collect::<Vec<_>>(),
            [
                "file/report.txt",
                &format!("file/report-{expected_hash}.txt"),
                "file/report.txt",
            ]
        );
        assert_eq!(
            plan.output.entries[5].snippet.as_deref(),
            Some("file/clip-20260715-131415-heading.md")
        );
        assert_eq!(
            plan.output.entries[6].snippet.as_deref(),
            Some("file/clip-20260715-131415-heading-2.md")
        );
        assert_eq!(plan.files.len(), 4, "unique files only");
        assert_eq!(plan.output.file_confirmations().len(), 4);
        let aggregate_json =
            serde_json::to_value(&plan.output).expect("aggregate JSON");
        assert_eq!(aggregate_json["mode"], "history");
        assert_eq!(aggregate_json["entries"].as_array().unwrap().len(), 7);
        assert_eq!(aggregate_json["attachments"].as_array().unwrap().len(), 3);
        assert!(aggregate_json.get("snippet").is_none(), "{aggregate_json}");
        let created = plan.save().expect("save aggregate");
        assert_eq!(created.len(), 4);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn aggregate_save_cleans_up_files_after_a_later_failure() {
        let root = test_root("aggregate-cleanup");
        let vault = root.join("vault");
        fs::create_dir_all(&vault).expect("vault");
        fs::write(vault.join("file"), b"blocks snippet directory")
            .expect("blocking file");
        let image = root.join("image.png");
        fs::write(&image, b"image").expect("image source");
        let clipboards = vec![
            image.display().to_string(),
            "# Structured\n\ncontent".to_string(),
        ];
        let plan = plan_history(&vault, &clipboards, test_now()).expect("plan");
        let error = plan.save().expect_err("second save must fail");
        assert!(error.contains("removed clipboard files"), "{error}");
        assert!(!vault.join("img/image.png").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn aggregate_planner_does_not_alias_snippets_and_attachments() {
        let root = test_root("aggregate-cross-kind");
        let vault = root.join("vault");
        fs::create_dir_all(&vault).expect("vault");
        let text = "# Heading\n\nbody";
        let attachment = root.join("clip-20260715-131415-heading.md");
        fs::write(&attachment, format!("{text}\n")).expect("attachment source");
        let plan = plan_history(
            &vault,
            &[text.to_string(), attachment.display().to_string()],
            test_now(),
        )
        .expect("cross-kind aggregate");
        let hash = sha256_hex(format!("{text}\n").as_bytes());
        assert_eq!(
            plan.output.entries[0].snippet.as_deref(),
            Some("file/clip-20260715-131415-heading.md")
        );
        assert_eq!(
            plan.output.entries[1].attachments[0].saved,
            format!("file/clip-20260715-131415-heading-{}.md", &hash[..8])
        );
        assert_eq!(plan.files.len(), 2);
        fs::remove_dir_all(root).expect("cleanup");
    }

    fn create_clipy_fixture_schema(connection: &Connection) {
        connection
            .execute_batch(
                "CREATE TABLE pasteboardHistories (\
                   id TEXT PRIMARY KEY NOT NULL, updateAt INTEGER NOT NULL\
                 );\
                 CREATE TABLE pasteboardHistoryAssets (\
                   id TEXT PRIMARY KEY NOT NULL,\
                   pasteboardHistoryID TEXT NOT NULL,\
                   \"index\" INTEGER NOT NULL,\
                   pasteboardType TEXT NOT NULL, data BLOB NOT NULL\
                 );",
            )
            .expect("Clipy fixture schema");
    }

    fn insert_clipy_asset(
        connection: &Connection,
        id: &str,
        history_id: &str,
        index: i64,
        pasteboard_type: &str,
        data: &[u8],
    ) {
        connection
            .execute(
                "INSERT INTO pasteboardHistoryAssets \
                 (id, pasteboardHistoryID, \"index\", pasteboardType, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (id, history_id, index, pasteboard_type, data),
            )
            .expect("Clipy fixture asset");
    }
}
