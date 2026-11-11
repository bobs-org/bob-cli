//! Lazy per-folder index of Record Notes.
//!
//! One [`RecordCatalog`] is scoped to a caller-owned invocation. It reads only
//! a requested folder's immediate Markdown files, and only through the closing
//! frontmatter delimiter. Missing folders are empty and are never created.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Component, Path, PathBuf},
    thread,
    time::SystemTime,
};

use serde_yaml::{Mapping, Value};

use super::schema::{RecordDiagnostic, RecordDiagnosticCode};
use crate::native::capture_block_ids::wikilink_phrase;
use crate::native::note_tasks::bounded_levenshtein;

const FRONTMATTER_BUFFER: usize = 8 * 1024;
const NEAREST_DISTANCE: usize = 2;
const NEAREST_LIMIT: usize = 3;

/// Caller-owned, lazy per-folder index.
#[derive(Debug)]
pub(crate) struct RecordCatalog {
    vault_root: PathBuf,
    folders: HashMap<String, FolderIndex>,
    files_opened: usize,
}

/// Immediate Markdown files discovered in one record folder.
#[derive(Debug, Clone)]
pub(crate) struct FolderIndex {
    directory: String,
    notes: Vec<IndexedRecord>,
    occupants: Vec<OccupiedRecordPath>,
    diagnostics: Vec<RecordDiagnostic>,
    files_opened: usize,
    create_refusal: Option<String>,
}

/// A Markdown-named filesystem entry that reserves a record path even when
/// it cannot be indexed as a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OccupiedRecordPath {
    pub(crate) relative_path: PathBuf,
    pub(crate) stem: String,
}

/// One Markdown file in a record folder, even when YAML or title is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedRecord {
    pub(crate) relative_path: PathBuf,
    pub(crate) stem: String,
    pub(crate) title: Option<String>,
    pub(crate) display: String,
    pub(crate) fields: Vec<IndexedField>,
    pub(crate) diagnostics: Vec<RecordDiagnostic>,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedField {
    pub(crate) key: String,
    pub(crate) value: IndexedFieldValue,
    /// Lossless YAML value for checkers; suggestions use `value` only.
    pub(crate) raw_value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexedFieldValue {
    Scalar(String),
    List(Vec<String>),
    Null,
    Unsupported,
}

/// Case-insensitive exact stem lookup for a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StemLookup<'a> {
    Exact(&'a IndexedRecord),
    Ambiguous(Vec<&'a IndexedRecord>),
    None,
}

/// Collapsed value suggestion for one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValueSuggestion {
    pub(crate) insert: String,
    pub(crate) count: usize,
    fold: String,
}

impl ValueSuggestion {
    /// Display spelling that collapsed aliases share (wikilink phrase or
    /// the original text).
    pub(crate) fn display(&self) -> String {
        value_display(&self.insert)
    }
}

/// A nonblocking near-miss hint for an authored open-text value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NearValueHint {
    pub(crate) field: String,
    pub(crate) authored: String,
    pub(crate) proposed: String,
    pub(crate) source_range: std::ops::Range<usize>,
}

impl RecordCatalog {
    pub(crate) fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: vault_root.into(),
            folders: HashMap::new(),
            files_opened: 0,
        }
    }

    /// Load `directory` once and reuse it for the rest of this invocation.
    pub(crate) fn folder(&mut self, directory: &str) -> &FolderIndex {
        if !self.folders.contains_key(directory) {
            let folder = load_folder(&self.vault_root, directory);
            self.files_opened += folder.files_opened;
            self.folders.insert(directory.to_owned(), folder);
        }
        self.folders
            .get(directory)
            .expect("folder was inserted above")
    }

    pub(crate) fn is_loaded(&self, directory: &str) -> bool {
        self.folders.contains_key(directory)
    }

    #[cfg(test)]
    pub(crate) fn loaded_directories(&self) -> Vec<String> {
        let mut directories: Vec<String> =
            self.folders.keys().cloned().collect();
        directories.sort();
        directories
    }

    pub(crate) fn files_opened(&self) -> usize {
        self.files_opened
    }

    pub(crate) fn vault_root(&self) -> &std::path::Path {
        &self.vault_root
    }
}

impl FolderIndex {
    pub(crate) fn directory(&self) -> &str {
        &self.directory
    }

    pub(crate) fn notes(&self) -> &[IndexedRecord] {
        &self.notes
    }

    pub(crate) fn diagnostics(&self) -> &[RecordDiagnostic] {
        &self.diagnostics
    }

    pub(crate) fn files_opened(&self) -> usize {
        self.files_opened
    }

    pub(crate) fn lookup_stem(&self, stem: &str) -> StemLookup<'_> {
        let folded = fold_key(stem);
        let matches: Vec<&IndexedRecord> = self
            .notes
            .iter()
            .filter(|note| fold_key(&note.stem) == folded)
            .collect();
        match matches.len() {
            0 => StemLookup::None,
            1 => StemLookup::Exact(matches[0]),
            _ => StemLookup::Ambiguous(matches),
        }
    }

    pub(crate) fn nearest_stems(&self, stem: &str) -> Vec<&IndexedRecord> {
        let folded = fold_key(stem);
        if folded.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, &IndexedRecord)> = self
            .notes
            .iter()
            .filter_map(|note| {
                let candidate = fold_key(&note.stem);
                if candidate == folded {
                    return None;
                }
                bounded_levenshtein(&folded, &candidate, NEAREST_DISTANCE)
                    .map(|distance| (distance, note))
            })
            .collect();
        scored.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.stem.cmp(&right.1.stem))
                .then_with(|| left.1.relative_path.cmp(&right.1.relative_path))
        });
        scored
            .into_iter()
            .take(NEAREST_LIMIT)
            .map(|(_, note)| note)
            .collect()
    }

    pub(crate) fn value_suggestions(
        &self,
        field: &str,
    ) -> Vec<ValueSuggestion> {
        suggestions_for(self.notes.iter(), field)
    }

    /// Near-miss hint for an authored open-text value: a different display
    /// spelling within edit distance 2 that has greater folder frequency.
    pub(crate) fn near_value_hint(
        &self,
        field: &str,
        authored: &str,
        source_range: std::ops::Range<usize>,
    ) -> Option<NearValueHint> {
        near_value_hint(self, field, authored, source_range)
    }

    pub(crate) fn nearest_values(
        &self,
        field: &str,
        query: &str,
    ) -> Vec<ValueSuggestion> {
        let folded = fold_key(query);
        if folded.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, ValueSuggestion)> = self
            .value_suggestions(field)
            .into_iter()
            .filter_map(|suggestion| {
                let insert_fold = fold_key(&suggestion.insert);
                let distance = bounded_levenshtein(
                    &folded,
                    &suggestion.fold,
                    NEAREST_DISTANCE,
                )
                .into_iter()
                .chain(bounded_levenshtein(
                    &folded,
                    &insert_fold,
                    NEAREST_DISTANCE,
                ))
                .min()?;
                Some((distance, suggestion))
            })
            .collect();
        scored.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| right.1.count.cmp(&left.1.count))
                .then_with(|| left.1.insert.cmp(&right.1.insert))
        });
        scored
            .into_iter()
            .take(NEAREST_LIMIT)
            .map(|(_, suggestion)| suggestion)
            .collect()
    }

    /// On-disk or caller-reserved path that collides with `stem`.
    pub(crate) fn colliding_path(
        &self,
        stem: &str,
        reserved: &[PathBuf],
    ) -> Option<PathBuf> {
        match self.lookup_stem(stem) {
            StemLookup::Exact(note) => Some(note.relative_path.clone()),
            StemLookup::Ambiguous(notes) => {
                Some(notes[0].relative_path.clone())
            }
            StemLookup::None => self
                .occupants
                .iter()
                .find(|path| fold_key(&path.stem) == fold_key(stem))
                .map(|path| path.relative_path.clone())
                .or_else(|| {
                    reserved.iter().find_map(|path| {
                        reserved_matches(&self.directory, stem, path)
                            .then(|| path.clone())
                    })
                }),
        }
    }

    pub(crate) fn create_refusal(&self) -> Option<&str> {
        self.create_refusal.as_deref()
    }
}

fn reserved_matches(directory: &str, stem: &str, reserved: &Path) -> bool {
    let Some(file_name) = reserved.file_name().and_then(|name| name.to_str())
    else {
        return false;
    };
    let Some(reserved_stem) = markdown_stem(file_name) else {
        return false;
    };
    if fold_key(reserved_stem) != fold_key(stem) {
        return false;
    }
    let Some(parent) = reserved.parent() else {
        return false;
    };
    parent == Path::new(directory)
        || parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| fold_key(name) == fold_key(directory))
}

fn empty_folder(
    directory: &str,
    diagnostics: Vec<RecordDiagnostic>,
) -> FolderIndex {
    FolderIndex {
        directory: directory.to_owned(),
        notes: Vec::new(),
        occupants: Vec::new(),
        diagnostics,
        files_opened: 0,
        create_refusal: None,
    }
}

fn unusable_folder(directory: &str, message: String) -> FolderIndex {
    FolderIndex {
        directory: directory.to_owned(),
        notes: Vec::new(),
        occupants: Vec::new(),
        diagnostics: vec![RecordDiagnostic::note(
            None,
            RecordDiagnosticCode::NoteRead,
            message.clone(),
        )],
        files_opened: 0,
        create_refusal: Some(message),
    }
}

fn load_folder(vault_root: &Path, directory: &str) -> FolderIndex {
    if !is_safe_segment(directory) {
        return unusable_folder(
            directory,
            format!("record folder {directory:?} is not a single safe segment"),
        );
    }

    let folder_path = vault_root.join(directory);
    let metadata = match fs::symlink_metadata(&folder_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return empty_folder(directory, Vec::new());
        }
        Err(error) => {
            return unusable_folder(
                directory,
                format!(
                    "could not read record folder {}: {error}",
                    folder_path.display()
                ),
            );
        }
    };
    if !metadata.is_dir() && !metadata.file_type().is_symlink() {
        return unusable_folder(
            directory,
            format!(
                "record folder {} is not a directory",
                folder_path.display()
            ),
        );
    }

    let Some(vault_canon) = fs::canonicalize(vault_root).ok() else {
        return unusable_folder(
            directory,
            format!("could not resolve vault root {}", vault_root.display()),
        );
    };
    let Some(folder_canon) = fs::canonicalize(&folder_path).ok() else {
        return unusable_folder(
            directory,
            format!(
                "could not resolve record folder {}",
                folder_path.display()
            ),
        );
    };
    if !is_within(&folder_canon, &vault_canon) {
        return unusable_folder(
            directory,
            format!(
                "record folder {} resolves outside the vault",
                folder_path.display()
            ),
        );
    }

    let entries = match fs::read_dir(&folder_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return empty_folder(directory, Vec::new());
        }
        Err(error) => {
            return unusable_folder(
                directory,
                format!(
                    "could not list record folder {}: {error}",
                    folder_path.display()
                ),
            );
        }
    };

    let mut notes = Vec::new();
    let mut occupants = Vec::new();
    let mut pending = Vec::new();
    let mut diagnostics = Vec::new();
    let mut create_refusal = None;
    let mut files_opened = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                let message = format!(
                    "could not read an entry in {}: {error}",
                    folder_path.display()
                );
                diagnostics.push(RecordDiagnostic::note(
                    None,
                    RecordDiagnosticCode::NoteRead,
                    message.clone(),
                ));
                create_refusal.get_or_insert(message);
                continue;
            }
        };
        let file_name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        let Some(stem) = markdown_stem(&file_name).map(str::to_owned) else {
            continue;
        };
        let path = entry.path();
        let relative_path = PathBuf::from(directory).join(&file_name);
        occupants.push(OccupiedRecordPath {
            relative_path: relative_path.clone(),
            stem: stem.clone(),
        });
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                notes.push(unreadable_note(
                    directory,
                    &file_name,
                    stem,
                    format!("could not inspect {}: {error}", path.display()),
                ));
                continue;
            }
        };
        if file_type.is_symlink() {
            match fs::canonicalize(&path) {
                Ok(target)
                    if target.is_file()
                        && is_within(&target, &folder_canon) => {}
                Ok(_) => {
                    diagnostics.push(RecordDiagnostic::note(
                        None,
                        RecordDiagnosticCode::NoteRead,
                        format!(
                            "record path {} is occupied by a symlink outside the record folder",
                            relative_path.display()
                        ),
                    ));
                    continue;
                }
                Err(error) => {
                    diagnostics.push(RecordDiagnostic::note(
                        None,
                        RecordDiagnosticCode::NoteRead,
                        format!(
                            "could not resolve occupied record symlink {}: {error}",
                            path.display()
                        ),
                    ));
                    continue;
                }
            }
        } else if !file_type.is_file() {
            diagnostics.push(RecordDiagnostic::note(
                None,
                RecordDiagnosticCode::NoteRead,
                format!(
                    "record path {} is occupied by a non-file entry",
                    relative_path.display()
                ),
            ));
            continue;
        }

        files_opened += 1;
        let modified =
            entry.metadata().ok().and_then(|meta| meta.modified().ok());
        pending.push(PendingFile {
            file_name,
            stem,
            path,
            modified,
        });
    }
    notes.extend(index_pending(directory, pending));
    notes.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    occupants
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    FolderIndex {
        directory: directory.to_owned(),
        notes,
        occupants,
        diagnostics,
        files_opened,
        create_refusal,
    }
}

struct PendingFile {
    file_name: String,
    stem: String,
    path: PathBuf,
    modified: Option<SystemTime>,
}

const PARALLEL_INDEX_THRESHOLD: usize = 32;

fn index_pending(
    directory: &str,
    pending: Vec<PendingFile>,
) -> Vec<IndexedRecord> {
    if pending.len() < PARALLEL_INDEX_THRESHOLD {
        return pending
            .into_iter()
            .map(|file| {
                index_file(
                    directory,
                    &file.file_name,
                    file.stem,
                    &file.path,
                    file.modified,
                )
            })
            .collect();
    }
    let workers = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 8)
        .min(pending.len());
    let chunk_size = pending.len().div_ceil(workers);
    thread::scope(|scope| {
        let mut handles = Vec::new();
        let mut rest = pending;
        while !rest.is_empty() {
            let remaining = if rest.len() > chunk_size {
                rest.split_off(chunk_size)
            } else {
                Vec::new()
            };
            let chunk = rest;
            rest = remaining;
            handles.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|file| {
                        index_file(
                            directory,
                            &file.file_name,
                            file.stem,
                            &file.path,
                            file.modified,
                        )
                    })
                    .collect::<Vec<_>>()
            }));
        }
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("record index worker"))
            .collect()
    })
}

fn unreadable_note(
    directory: &str,
    file_name: &str,
    stem: String,
    message: String,
) -> IndexedRecord {
    IndexedRecord {
        relative_path: PathBuf::from(directory).join(file_name),
        display: stem.clone(),
        title: None,
        stem,
        fields: Vec::new(),
        diagnostics: vec![RecordDiagnostic::note(
            None,
            RecordDiagnosticCode::NoteRead,
            message,
        )],
        modified: None,
    }
}

fn index_file(
    directory: &str,
    file_name: &str,
    stem: String,
    path: &Path,
    modified: Option<SystemTime>,
) -> IndexedRecord {
    let relative_path = PathBuf::from(directory).join(file_name);
    let mut diagnostics = Vec::new();
    let frontmatter = match read_through_frontmatter(path) {
        Ok(frontmatter) => frontmatter,
        Err(error) => {
            diagnostics.push(RecordDiagnostic::note(
                None,
                RecordDiagnosticCode::NoteRead,
                format!("could not read {}: {error}", relative_path.display()),
            ));
            return IndexedRecord {
                relative_path,
                display: stem.clone(),
                title: None,
                stem,
                fields: Vec::new(),
                diagnostics,
                modified,
            };
        }
    };
    indexed_record_from_frontmatter(relative_path, stem, frontmatter, modified)
}

/// Project one staged record postimage into the shared catalog member shape
/// without disk I/O. The frontmatter decoder matches disk entries exactly:
/// only bytes through the closing delimiter are parsed, and unreadable or
/// invalid members stay visible as structural members.
pub(crate) fn staged_record(
    relative_path: PathBuf,
    stem: String,
    markdown: &str,
) -> IndexedRecord {
    let frontmatter = frontmatter_from_markdown(markdown);
    indexed_record_from_frontmatter(relative_path, stem, frontmatter, None)
}

fn indexed_record_from_frontmatter(
    relative_path: PathBuf,
    stem: String,
    frontmatter: Option<String>,
    modified: Option<SystemTime>,
) -> IndexedRecord {
    let mut diagnostics = Vec::new();
    let Some(yaml) = frontmatter else {
        diagnostics.push(RecordDiagnostic::note(
            Some("title"),
            RecordDiagnosticCode::MissingTitle,
            "record is missing a title",
        ));
        return IndexedRecord {
            relative_path,
            display: stem.clone(),
            title: None,
            stem,
            fields: Vec::new(),
            diagnostics,
            modified,
        };
    };
    match serde_yaml::from_str::<Value>(&yaml) {
        Ok(Value::Mapping(mapping)) => record_from_mapping(
            relative_path,
            stem,
            mapping,
            diagnostics,
            modified,
        ),
        Ok(_) => {
            diagnostics.push(RecordDiagnostic::note(
                None,
                RecordDiagnosticCode::InvalidYaml,
                "record frontmatter must be a mapping",
            ));
            diagnostics.push(RecordDiagnostic::note(
                Some("title"),
                RecordDiagnosticCode::MissingTitle,
                "record is missing a title",
            ));
            IndexedRecord {
                relative_path,
                display: stem.clone(),
                title: None,
                stem,
                fields: Vec::new(),
                diagnostics,
                modified,
            }
        }
        Err(error) => {
            diagnostics.push(RecordDiagnostic::note(
                None,
                RecordDiagnosticCode::InvalidYaml,
                format!("could not parse YAML: {error}"),
            ));
            diagnostics.push(RecordDiagnostic::note(
                Some("title"),
                RecordDiagnosticCode::MissingTitle,
                "record is missing a title",
            ));
            IndexedRecord {
                relative_path,
                display: stem.clone(),
                title: None,
                stem,
                fields: Vec::new(),
                diagnostics,
                modified,
            }
        }
    }
}

fn record_from_mapping(
    relative_path: PathBuf,
    stem: String,
    mapping: Mapping,
    mut diagnostics: Vec<RecordDiagnostic>,
    modified: Option<SystemTime>,
) -> IndexedRecord {
    let mut fields = Vec::with_capacity(mapping.len());
    let mut title = None;
    for (key, value) in mapping {
        let Some(name) = key.as_str() else {
            continue;
        };
        if name == "title" {
            title = scalar_text(&value).filter(|text| !text.is_empty());
        }
        fields.push(IndexedField {
            key: name.to_owned(),
            value: field_value(&value),
            raw_value: value,
        });
    }
    if title.is_none() {
        diagnostics.push(RecordDiagnostic::note(
            Some("title"),
            RecordDiagnosticCode::MissingTitle,
            "record is missing a title",
        ));
    }
    let display = title.clone().unwrap_or_else(|| stem.clone());
    IndexedRecord {
        relative_path,
        stem,
        title,
        display,
        fields,
        diagnostics,
        modified,
    }
}

fn field_value(value: &Value) -> IndexedFieldValue {
    match value {
        Value::Null => IndexedFieldValue::Null,
        Value::Sequence(items) => IndexedFieldValue::List(
            items.iter().filter_map(scalar_text).collect(),
        ),
        Value::Mapping(_) | Value::Tagged(_) => IndexedFieldValue::Unsupported,
        other => scalar_text(other)
            .map(IndexedFieldValue::Scalar)
            .unwrap_or(IndexedFieldValue::Unsupported),
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(true) => Some("true".to_owned()),
        Value::Bool(false) => Some("false".to_owned()),
        Value::Null
        | Value::Sequence(_)
        | Value::Mapping(_)
        | Value::Tagged(_) => None,
    }
}

fn read_through_frontmatter(path: &Path) -> std::io::Result<Option<String>> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(FRONTMATTER_BUFFER, file);
    let mut first = String::new();
    if reader.read_line(&mut first)? == 0 {
        return Ok(None);
    }
    if trim_line_ending(&first) != "---" {
        return Ok(None);
    }
    let mut yaml = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if trim_line_ending(&line) == "---" {
            return Ok(Some(yaml));
        }
        yaml.push_str(&line);
    }
}

fn trim_line_ending(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

/// String twin of [`read_through_frontmatter`]: return the YAML between the
/// first `---` line and its closing delimiter, or `None` when the note has
/// no frontmatter block. Only bytes through the closing delimiter are
/// ever parsed, exactly as on disk.
fn frontmatter_from_markdown(markdown: &str) -> Option<String> {
    let mut lines = markdown.split_inclusive('\n');
    let first = lines.next()?;
    if trim_line_ending(first) != "---" {
        return None;
    }
    let mut yaml = String::new();
    for line in lines {
        if trim_line_ending(line) == "---" {
            return Some(yaml);
        }
        yaml.push_str(line);
    }
    None
}

fn markdown_stem(filename: &str) -> Option<&str> {
    let (stem, extension) = filename.rsplit_once('.')?;
    (extension.eq_ignore_ascii_case("md") && !stem.is_empty()).then_some(stem)
}

fn is_safe_segment(directory: &str) -> bool {
    if directory.is_empty() || directory.contains('\0') {
        return false;
    }
    let mut components = Path::new(directory).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}

fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn fold_key(value: &str) -> String {
    value.to_lowercase()
}

fn near_value_hint(
    folder: &FolderIndex,
    field: &str,
    authored: &str,
    source_range: std::ops::Range<usize>,
) -> Option<NearValueHint> {
    let authored = authored.trim();
    if authored.is_empty() {
        return None;
    }
    let authored_fold = fold_key(&value_display(authored));
    let authored_insert_fold = fold_key(authored);
    let suggestions = folder.value_suggestions(field);
    let authored_count = suggestions
        .iter()
        .find(|suggestion| {
            suggestion.fold == authored_fold
                || fold_key(&suggestion.insert) == authored_insert_fold
        })
        .map(|suggestion| suggestion.count)
        .unwrap_or(0);
    let mut best: Option<(usize, &ValueSuggestion)> = None;
    for suggestion in &suggestions {
        if suggestion.fold == authored_fold
            || fold_key(&suggestion.insert) == authored_insert_fold
            || fold_key(&suggestion.display()) == authored_fold
        {
            continue;
        }
        if suggestion.count <= authored_count {
            continue;
        }
        let Some(distance) = bounded_levenshtein(
            &authored_fold,
            &suggestion.fold,
            NEAREST_DISTANCE,
        )
        .into_iter()
        .chain(bounded_levenshtein(
            &authored_insert_fold,
            &fold_key(&suggestion.insert),
            NEAREST_DISTANCE,
        ))
        .chain(bounded_levenshtein(
            &authored_fold,
            &fold_key(&suggestion.display()),
            NEAREST_DISTANCE,
        ))
        .min() else {
            continue;
        };
        if distance == 0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_distance, best_suggestion)) => {
                suggestion.count > best_suggestion.count
                    || (suggestion.count == best_suggestion.count
                        && (distance < best_distance
                            || (distance == best_distance
                                && suggestion.insert < best_suggestion.insert)))
            }
        };
        if better {
            best = Some((distance, suggestion));
        }
    }
    let (_, suggestion) = best?;
    Some(NearValueHint {
        field: field.to_owned(),
        authored: authored.to_owned(),
        proposed: suggestion.display(),
        source_range,
    })
}

fn value_display(original: &str) -> String {
    let trimmed = original.trim();
    if let Some(inner) = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        && !inner.contains("[[")
    {
        return wikilink_phrase(inner);
    }
    original.to_owned()
}

fn suggestions_for<'a>(
    notes: impl Iterator<Item = &'a IndexedRecord>,
    field: &str,
) -> Vec<ValueSuggestion> {
    #[derive(Clone)]
    struct OriginalVote {
        count: usize,
        recency: Option<SystemTime>,
        path: PathBuf,
    }
    struct Group {
        originals: HashMap<String, OriginalVote>,
        count: usize,
        recency: Option<SystemTime>,
        path: PathBuf,
    }

    let mut groups: HashMap<String, Group> = HashMap::new();
    for note in notes {
        let mut originals_by_fold: HashMap<String, HashMap<String, usize>> =
            HashMap::new();
        for member in field_members(note, field) {
            if member.is_empty() {
                continue;
            }
            let fold = fold_key(&value_display(member));
            *originals_by_fold
                .entry(fold)
                .or_default()
                .entry(member.to_owned())
                .or_insert(0) += 1;
        }
        for (fold, originals) in originals_by_fold {
            let winning_original = originals
                .into_iter()
                .max_by(|(left_text, left_count), (right_text, right_count)| {
                    left_count
                        .cmp(right_count)
                        .then_with(|| right_text.cmp(left_text))
                })
                .map(|(text, _)| text)
                .expect("fold group is non-empty");
            let group = groups.entry(fold).or_insert_with(|| Group {
                originals: HashMap::new(),
                count: 0,
                recency: None,
                path: note.relative_path.clone(),
            });
            group.count += 1;
            if recency_newer(note.modified, group.recency) {
                group.recency = note.modified;
                group.path = note.relative_path.clone();
            } else if group.recency == note.modified
                && note.relative_path < group.path
            {
                group.path = note.relative_path.clone();
            }
            let vote =
                group.originals.entry(winning_original).or_insert_with(|| {
                    OriginalVote {
                        count: 0,
                        recency: None,
                        path: note.relative_path.clone(),
                    }
                });
            vote.count += 1;
            if recency_newer(note.modified, vote.recency) {
                vote.recency = note.modified;
                vote.path = note.relative_path.clone();
            } else if vote.recency == note.modified
                && note.relative_path < vote.path
            {
                vote.path = note.relative_path.clone();
            }
        }
    }

    let mut suggestions: Vec<(ValueSuggestion, Option<SystemTime>, PathBuf)> =
        groups
            .into_iter()
            .map(|(fold, group)| {
                let insert = group
                    .originals
                    .into_iter()
                    .max_by(|(left_text, left), (right_text, right)| {
                        left.count
                            .cmp(&right.count)
                            .then_with(|| {
                                recency_cmp(right.recency, left.recency)
                            })
                            .then_with(|| left.path.cmp(&right.path).reverse())
                            .then_with(|| right_text.cmp(left_text))
                    })
                    .map(|(text, _)| text)
                    .expect("group has at least one original");
                (
                    ValueSuggestion {
                        insert,
                        count: group.count,
                        fold,
                    },
                    group.recency,
                    group.path,
                )
            })
            .collect();
    suggestions.sort_by(|left, right| {
        right
            .0
            .count
            .cmp(&left.0.count)
            .then_with(|| recency_cmp(left.1, right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.0.insert.cmp(&right.0.insert))
    });
    suggestions
        .into_iter()
        .map(|(suggestion, _, _)| suggestion)
        .collect()
}

fn field_members<'a>(note: &'a IndexedRecord, field: &str) -> Vec<&'a str> {
    note.fields
        .iter()
        .filter(|entry| entry.key == field)
        .flat_map(|entry| match &entry.value {
            IndexedFieldValue::Scalar(text) => vec![text.as_str()],
            IndexedFieldValue::List(values) => {
                values.iter().map(String::as_str).collect()
            }
            IndexedFieldValue::Null | IndexedFieldValue::Unsupported => {
                Vec::new()
            }
        })
        .collect()
}

fn recency_newer(
    candidate: Option<SystemTime>,
    current: Option<SystemTime>,
) -> bool {
    match (candidate, current) {
        (Some(candidate), Some(current)) => candidate > current,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

fn recency_cmp(
    left: Option<SystemTime>,
    right: Option<SystemTime>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

impl IndexedRecord {
    pub(crate) fn field(&self, key: &str) -> Option<&IndexedFieldValue> {
        self.fields
            .iter()
            .find(|field| field.key == key)
            .map(|field| &field.value)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, SystemTime},
    };

    use super::{
        markdown_stem, read_through_frontmatter, FolderIndex,
        IndexedFieldValue, IndexedRecord, RecordCatalog, StemLookup,
        ValueSuggestion,
    };
    use crate::native::records::schema::RecordDiagnosticCode;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bob-records-catalog-{}-{}",
                process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp vault");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }

    fn write_note(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).expect("create folder");
        let path = dir.join(name);
        fs::write(&path, body).expect("write note");
        path
    }

    fn set_mtime(path: &Path, seconds: u64) {
        let file = fs::File::options()
            .write(true)
            .open(path)
            .expect("open for mtime");
        file.set_modified(
            SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
        )
        .expect("set mtime");
    }

    fn catalog_at(root: &Path) -> RecordCatalog {
        RecordCatalog::new(root)
    }

    fn eat(catalog: &mut RecordCatalog) -> &FolderIndex {
        catalog.folder("eat")
    }

    fn inserts(suggestions: &[ValueSuggestion]) -> Vec<&str> {
        suggestions
            .iter()
            .map(|suggestion| suggestion.insert.as_str())
            .collect()
    }

    fn restaurant(title: &str, location: &str) -> String {
        format!(
            "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\ntitle: {title:?}\nstatus: liked\nlocation: {location}\n---\n# {title}\n\n## Notes\n\nbody must not rank\nlocation: ignored-in-body\n"
        )
    }

    #[test]
    fn missing_and_empty_directories_are_empty_and_are_not_created() {
        let root = TempDir::new();
        let missing = root.path().join("eat");
        assert!(!missing.exists());
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        assert!(folder.notes().is_empty());
        assert!(folder.diagnostics().is_empty());
        assert!(!missing.exists());

        fs::create_dir_all(root.path().join("podcasts")).unwrap();
        let podcasts = catalog.folder("podcasts");
        assert!(podcasts.notes().is_empty());
        assert!(root.path().join("podcasts").is_dir());
    }

    #[test]
    fn short_names_unicode_extensions_and_directories_are_safe() {
        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        fs::create_dir_all(&eat_dir).unwrap();
        fs::write(eat_dir.join("a"), "ordinary file").unwrap();
        fs::write(eat_dir.join("ab"), "ordinary file").unwrap();
        fs::write(eat_dir.join("é.txt"), "ordinary file").unwrap();
        fs::write(eat_dir.join(".md"), "empty stem").unwrap();
        fs::create_dir(eat_dir.join("a.md")).unwrap();
        fs::create_dir(eat_dir.join("ab.md")).unwrap();
        write_note(&eat_dir, "Upper.MD", &restaurant("Upper", "Clark"));
        write_note(&eat_dir, "Crème.md", &restaurant("Crème", "Cranford"));
        write_note(&eat_dir, "東京.MD", &restaurant("Tokyo", "Tokyo"));

        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        let stems: Vec<&str> = folder
            .notes()
            .iter()
            .map(|note| note.stem.as_str())
            .collect();
        assert_eq!(stems, ["Crème", "Upper", "東京"]);
        assert_eq!(
            folder.colliding_path("A", &[]),
            Some(PathBuf::from("eat/a.md"))
        );
        assert_eq!(
            folder.colliding_path("ab", &[]),
            Some(PathBuf::from("eat/ab.md"))
        );
    }

    #[test]
    fn two_consumers_reuse_one_read_and_skip_unrelated_folders() {
        let root = TempDir::new();
        write_note(
            &root.path().join("eat"),
            "rush_bowl.md",
            &restaurant("Rush Bowl", "Clark"),
        );
        write_note(
            &root.path().join("podcasts"),
            "talk_python.md",
            "---\ntitle: Talk Python to Me\nstatus: subscribed\n---\n# Talk Python\n",
        );
        let mut catalog = catalog_at(root.path());
        assert_eq!(eat(&mut catalog).notes().len(), 1);
        let opened = catalog.files_opened();
        assert_eq!(opened, 1);
        assert_eq!(eat(&mut catalog).notes().len(), 1);
        assert_eq!(catalog.files_opened(), opened);
        assert!(catalog.is_loaded("eat"));
        assert!(!catalog.is_loaded("podcasts"));
        assert_eq!(catalog.folder("podcasts").notes().len(), 1);
        assert_eq!(catalog.files_opened(), opened + 1);
    }

    #[test]
    fn malformed_yaml_and_missing_title_still_collide_on_stem() {
        let root = TempDir::new();
        write_note(
            &root.path().join("eat"),
            "broken.md",
            "---\n: : not yaml\n---\n# Broken\n",
        );
        write_note(
            &root.path().join("eat"),
            "untitled.md",
            "---\nstatus: liked\n---\n# Untitled\n",
        );
        write_note(&root.path().join("eat"), "nobody.md", "# just a heading\n");
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        assert_eq!(folder.notes().len(), 3);
        for stem in ["broken", "untitled", "nobody"] {
            let StemLookup::Exact(note) = folder.lookup_stem(stem) else {
                panic!("expected exact stem {stem}");
            };
            assert_eq!(note.display, stem);
            assert!(note.title.is_none());
            assert!(note.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == RecordDiagnosticCode::MissingTitle
            }));
        }
        assert!(folder.lookup_stem("broken").is_exact());
        assert_eq!(
            folder.colliding_path("untitled", &[]),
            Some(PathBuf::from("eat/untitled.md"))
        );
        let broken = match folder.lookup_stem("broken") {
            StemLookup::Exact(note) => note,
            other => panic!("expected exact, got {other:?}"),
        };
        assert!(broken.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == RecordDiagnosticCode::InvalidYaml
        }));
    }

    #[test]
    fn scalar_and_list_values_count_each_record_once() {
        let root = TempDir::new();
        write_note(
            &root.path().join("eat"),
            "one.md",
            "---\ntitle: One\nlocation: Westfield\ncompanions:\n  - \"[[kelly]]\"\n  - \"[[kelly]]\"\n  - \"[[alice]]\"\n---\n# One\n",
        );
        write_note(
            &root.path().join("eat"),
            "two.md",
            "---\ntitle: Two\nlocation: Westfield\ncompanions: [\"[[kelly]]\"]\n---\n# Two\n",
        );
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        let locations = folder.value_suggestions("location");
        assert_eq!(inserts(&locations), ["Westfield"]);
        assert_eq!(locations[0].count, 2);
        let companions = folder.value_suggestions("companions");
        assert_eq!(
            companions
                .iter()
                .find(|row| row.insert == "[[kelly]]")
                .unwrap()
                .count,
            2
        );
        assert_eq!(
            companions
                .iter()
                .find(|row| row.insert == "[[alice]]")
                .unwrap()
                .count,
            1
        );
        let one = match folder.lookup_stem("one") {
            StemLookup::Exact(note) => note,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            one.field("companions"),
            Some(&IndexedFieldValue::List(vec![
                "[[kelly]]".into(),
                "[[kelly]]".into(),
                "[[alice]]".into(),
            ]))
        );
    }

    #[test]
    fn duplicate_display_aliases_collapse_to_the_most_used_spelling() {
        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        for index in 0..15 {
            let path = write_note(
                &eat_dir,
                &format!("west_{index}.md"),
                &restaurant(&format!("West {index}"), "Westfield"),
            );
            set_mtime(&path, 1_000 + index);
        }
        for index in 0..5 {
            let path = write_note(
                &eat_dir,
                &format!("link_{index}.md"),
                &restaurant(
                    &format!("Link {index}"),
                    "\"[[nj_towns#^z-241117-0d|westfield]]\"",
                ),
            );
            set_mtime(&path, 2_000 + index);
        }
        let mut catalog = catalog_at(root.path());
        let suggestions = eat(&mut catalog).value_suggestions("location");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].insert, "Westfield");
        assert_eq!(suggestions[0].count, 20);
    }

    #[test]
    fn frequency_then_recency_then_path_break_suggestion_ties() {
        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        let cranford =
            write_note(&eat_dir, "aaa.md", &restaurant("Aaa", "Cranford"));
        let clark = write_note(&eat_dir, "zzz.md", &restaurant("Zzz", "Clark"));
        set_mtime(&cranford, 10);
        set_mtime(&clark, 20);
        let mut catalog = catalog_at(root.path());
        assert_eq!(
            inserts(&eat(&mut catalog).value_suggestions("location")),
            ["Clark", "Cranford"]
        );

        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        let late_west =
            write_note(&eat_dir, "b.md", &restaurant("B", "WESTFIELD"));
        let early_west =
            write_note(&eat_dir, "a.md", &restaurant("A", "Westfield"));
        set_mtime(&early_west, 10);
        set_mtime(&late_west, 50);
        let mut catalog = catalog_at(root.path());
        let suggestions = eat(&mut catalog).value_suggestions("location");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].insert, "WESTFIELD");
        assert_eq!(suggestions[0].count, 2);

        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        let first = write_note(&eat_dir, "a.md", &restaurant("A", "Cranford"));
        let second = write_note(&eat_dir, "b.md", &restaurant("B", "Clark"));
        set_mtime(&first, 7);
        set_mtime(&second, 7);
        let mut catalog = catalog_at(root.path());
        assert_eq!(
            inserts(&eat(&mut catalog).value_suggestions("location")),
            ["Cranford", "Clark"]
        );
    }

    #[test]
    fn unicode_stems_and_titles_round_trip_through_lookup() {
        let root = TempDir::new();
        write_note(
            &root.path().join("eat"),
            "café.md",
            "---\ntitle: Café Crème\nlocation: 東京\n---\n# Café Crème\n",
        );
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        let StemLookup::Exact(note) = folder.lookup_stem("Café") else {
            panic!("unicode stem should match case-insensitively");
        };
        assert_eq!(note.title.as_deref(), Some("Café Crème"));
        assert_eq!(note.display, "Café Crème");
        assert_eq!(inserts(&folder.value_suggestions("location")), ["東京"]);
    }

    #[test]
    fn case_collisions_are_ambiguous_and_keep_original_paths() {
        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        fs::create_dir_all(&eat_dir).unwrap();
        let upper = eat_dir.join("Alpha.md");
        let lower = eat_dir.join("alpha.md");
        let created_pair = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&upper)
            .is_ok()
            && fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lower)
                .is_ok();
        if created_pair {
            fs::write(&upper, restaurant("Alpha", "Clark")).unwrap();
            fs::write(&lower, restaurant("alpha", "Clark")).unwrap();
            let mut catalog = catalog_at(root.path());
            match eat(&mut catalog).lookup_stem("ALPHA") {
                StemLookup::Ambiguous(notes) => {
                    let stems: Vec<&str> =
                        notes.iter().map(|note| note.stem.as_str()).collect();
                    assert!(stems.contains(&"Alpha"));
                    assert!(stems.contains(&"alpha"));
                    assert_eq!(
                        notes[0].relative_path,
                        PathBuf::from("eat/Alpha.md")
                    );
                    assert_eq!(
                        notes[1].relative_path,
                        PathBuf::from("eat/alpha.md")
                    );
                }
                other => panic!("expected ambiguous lookup, got {other:?}"),
            }
        }

        let injected = FolderIndex {
            directory: "eat".into(),
            occupants: Vec::new(),
            notes: vec![
                IndexedRecord {
                    relative_path: PathBuf::from("eat/Alpha.md"),
                    stem: "Alpha".into(),
                    title: Some("Alpha".into()),
                    display: "Alpha".into(),
                    fields: Vec::new(),
                    diagnostics: Vec::new(),
                    modified: None,
                },
                IndexedRecord {
                    relative_path: PathBuf::from("eat/alpha.md"),
                    stem: "alpha".into(),
                    title: Some("alpha".into()),
                    display: "alpha".into(),
                    fields: Vec::new(),
                    diagnostics: Vec::new(),
                    modified: None,
                },
            ],
            diagnostics: Vec::new(),
            files_opened: 0,
            create_refusal: None,
        };
        match injected.lookup_stem("alpha") {
            StemLookup::Ambiguous(notes) => assert_eq!(notes.len(), 2),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn nearest_stems_and_values_honor_did_you_mean_cutoffs() {
        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        write_note(
            &eat_dir,
            "ninja_sushi.md",
            &restaurant("Ninja Sushi", "Clark"),
        );
        write_note(
            &eat_dir,
            "ninja_sush.md",
            &restaurant("Ninja Sush", "Clark"),
        );
        write_note(&eat_dir, "ram_nagi.md", &restaurant("Ram Nagi", "Clark"));
        write_note(
            &eat_dir,
            "unrelated.md",
            &restaurant("Unrelated", "Westfield"),
        );
        write_note(&eat_dir, "clark.md", &restaurant("Clark", "Clark"));
        write_note(&eat_dir, "clerk.md", &restaurant("Clerk", "Clerk"));
        write_note(
            &eat_dir,
            "cranford.md",
            &restaurant("Cranford", "Cranford"),
        );
        write_note(&eat_dir, "nyc.md", &restaurant("NYC", "NYC"));
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        let stems: Vec<&str> = folder
            .nearest_stems("ninja_sushy")
            .iter()
            .map(|note| note.stem.as_str())
            .collect();
        assert_eq!(stems, ["ninja_sush", "ninja_sushi"]);
        assert!(folder.nearest_stems("zzzzzzzz").is_empty());
        let nearest = folder.nearest_values("location", "Clrk");
        assert_eq!(inserts(&nearest), ["Clark", "Clerk"]);
        assert!(folder.nearest_values("location", "zzzzzzzz").is_empty());
        assert_eq!(folder.nearest_stems("ninja_sushi").len(), 1);
    }

    #[test]
    fn reserved_paths_collide_without_rescanning() {
        let root = TempDir::new();
        write_note(
            &root.path().join("eat"),
            "rush_bowl.md",
            &restaurant("Rush Bowl", "Clark"),
        );
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        assert_eq!(
            folder.colliding_path("Rush_Bowl", &[]),
            Some(PathBuf::from("eat/rush_bowl.md"))
        );
        assert_eq!(
            folder.colliding_path(
                "new_place",
                &[PathBuf::from("eat/New_Place.md")]
            ),
            Some(PathBuf::from("eat/New_Place.md"))
        );
        assert_eq!(
            folder
                .colliding_path("other", &[PathBuf::from("podcasts/other.md")]),
            None
        );
        let mut missing = catalog_at(root.path());
        assert_eq!(
            missing
                .folder("tv")
                .colliding_path("landman", &[PathBuf::from("tv/Landman.md")]),
            Some(PathBuf::from("tv/Landman.md"))
        );
    }

    #[test]
    fn outside_symlinks_are_not_followed_and_bodies_do_not_rank() {
        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        write_note(&eat_dir, "inside.md", &restaurant("Inside", "Clark"));
        let outside = root.path().join("outside.md");
        fs::write(
            &outside,
            "---\ntitle: Outside\nlocation: Westfield\n---\n# Outside\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&outside, eat_dir.join("escaped.md")).unwrap();
            symlink(eat_dir.join("inside.md"), eat_dir.join("alias.md"))
                .unwrap();
        }
        let nested = eat_dir.join("nested");
        fs::create_dir_all(&nested).unwrap();
        write_note(&nested, "child.md", &restaurant("Child", "NYC"));
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        let stems: Vec<&str> = folder
            .notes()
            .iter()
            .map(|note| note.stem.as_str())
            .collect();
        assert!(stems.contains(&"inside"));
        assert!(!stems.contains(&"escaped"));
        assert!(!stems.contains(&"child"));
        #[cfg(unix)]
        assert_eq!(
            folder.colliding_path("escaped", &[]),
            Some(PathBuf::from("eat/escaped.md"))
        );
        assert_eq!(inserts(&folder.value_suggestions("location")), ["Clark"]);
    }

    #[test]
    fn false_zero_and_null_are_preserved_as_raw_values() {
        let root = TempDir::new();
        write_note(
            &root.path().join("eat"),
            "flags.md",
            "---\ntitle: Flags\nlid: null\nflag: false\ncount: 0\n---\n# Flags\n",
        );
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        let note = match folder.lookup_stem("flags") {
            StemLookup::Exact(note) => note,
            other => panic!("{other:?}"),
        };
        assert_eq!(note.field("lid"), Some(&IndexedFieldValue::Null));
        assert_eq!(
            note.field("flag"),
            Some(&IndexedFieldValue::Scalar("false".into()))
        );
        assert_eq!(
            note.field("count"),
            Some(&IndexedFieldValue::Scalar("0".into()))
        );
        assert!(matches!(
            &note
                .fields
                .iter()
                .find(|field| field.key == "flag")
                .unwrap()
                .raw_value,
            serde_yaml::Value::Bool(false)
        ));
        assert!(matches!(
            &note
                .fields
                .iter()
                .find(|field| field.key == "count")
                .unwrap()
                .raw_value,
            serde_yaml::Value::Number(_)
        ));
        assert!(folder.value_suggestions("lid").is_empty());
    }

    #[test]
    fn unsupported_list_members_remain_raw_but_do_not_rank() {
        let root = TempDir::new();
        write_note(
            &root.path().join("eat"),
            "members.md",
            "---\ntitle: Members\ntags:\n  - useful\n  - null\n  - {bad: map}\n  - false\n  - 'false'\n---\n# Members\n",
        );
        let mut catalog = catalog_at(root.path());
        let folder = eat(&mut catalog);
        let note = match folder.lookup_stem("members") {
            StemLookup::Exact(note) => note,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            note.field("tags"),
            Some(&IndexedFieldValue::List(vec![
                "useful".into(),
                "false".into(),
                "false".into(),
            ]))
        );
        let raw = &note
            .fields
            .iter()
            .find(|field| field.key == "tags")
            .unwrap()
            .raw_value;
        let serde_yaml::Value::Sequence(members) = raw else {
            panic!("raw list value was retained: {raw:?}");
        };
        assert!(matches!(&members[1], serde_yaml::Value::Null));
        assert!(matches!(&members[2], serde_yaml::Value::Mapping(_)));
        assert!(matches!(&members[3], serde_yaml::Value::Bool(false)));
        assert!(
            matches!(&members[4], serde_yaml::Value::String(value) if value == "false")
        );
        let suggestions = folder.value_suggestions("tags");
        assert!(suggestions
            .iter()
            .any(|suggestion| suggestion.insert == "useful"));
        assert!(suggestions
            .iter()
            .any(|suggestion| suggestion.insert == "false"));
        assert!(suggestions
            .iter()
            .all(|suggestion| suggestion.insert != "map"));
    }

    #[test]
    fn frozen_fixture_notes_index_titles_and_stems() {
        let root = TempDir::new();
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/records/source");
        fs::create_dir_all(root.path().join("eat")).unwrap();
        fs::create_dir_all(root.path().join("podcasts")).unwrap();
        for name in ["rush_bowl.md", "brown_fox_coffee_box.md", "ahrres.md"] {
            fs::copy(
                source.join("eat").join(name),
                root.path().join("eat").join(name),
            )
            .unwrap();
        }
        fs::copy(
            source.join("podcasts/talk_python.md"),
            root.path().join("podcasts/talk_python.md"),
        )
        .unwrap();
        let mut catalog = catalog_at(root.path());
        {
            let folder = eat(&mut catalog);
            let rush = match folder.lookup_stem("rush_bowl") {
                StemLookup::Exact(note) => note,
                other => panic!("{other:?}"),
            };
            assert_eq!(rush.title.as_deref(), Some("Rush Bowl"));
            assert_eq!(
                rush.field("location"),
                Some(&IndexedFieldValue::Scalar(
                    "[[nj_towns#^z-241117-0b|cranford]]".into()
                ))
            );
            assert!(inserts(&folder.value_suggestions("location"))
                .contains(&"[[nj_towns#^z-241117-0b|cranford]]"));
        }
        let podcasts = catalog.folder("podcasts");
        let talk = match podcasts.lookup_stem("talk_python") {
            StemLookup::Exact(note) => note,
            other => panic!("{other:?}"),
        };
        assert_eq!(talk.title.as_deref(), Some("Talk Python to Me"));
        assert_eq!(talk.stem, "talk_python");
    }

    #[test]
    fn frontmatter_reader_stops_at_the_closing_delimiter() {
        let root = TempDir::new();
        let path = write_note(
            &root.path().join("eat"),
            "stop.md",
            "---\ntitle: Stop\n---\n# Stop\nthis is body\n",
        );
        let yaml = read_through_frontmatter(&path).unwrap().unwrap();
        assert_eq!(yaml, "title: Stop\n");
        assert_eq!(markdown_stem("Talk.MD"), Some("Talk"));
        assert_eq!(markdown_stem(".md"), None);
    }

    impl StemLookup<'_> {
        fn is_exact(&self) -> bool {
            matches!(self, Self::Exact(_))
        }
    }

    #[test]
    #[ignore = "records catalog timing evidence; wall-clock stays out of CI"]
    fn indexes_500_synthetic_notes_for_timing_evidence() {
        use std::time::Instant;

        let root = TempDir::new();
        let eat_dir = root.path().join("eat");
        fs::create_dir_all(&eat_dir).unwrap();
        for index in 0..500 {
            write_note(
                &eat_dir,
                &format!("place_{index}.md"),
                &restaurant(&format!("Place {index}"), "Westfield"),
            );
        }

        let mut cold_catalog = catalog_at(root.path());
        let cold_start = Instant::now();
        assert_eq!(eat(&mut cold_catalog).notes().len(), 500);
        let cold = cold_start.elapsed();

        let warm_start = Instant::now();
        assert_eq!(eat(&mut cold_catalog).notes().len(), 500);
        let warm_cached = warm_start.elapsed();

        let mut samples = Vec::new();
        for _ in 0..5 {
            let mut catalog = catalog_at(root.path());
            let start = Instant::now();
            assert_eq!(eat(&mut catalog).notes().len(), 500);
            samples.push(start.elapsed());
        }
        eprintln!(
            "records catalog 500 notes (athena release evidence): cold={cold:?} warm_cached={warm_cached:?} warm_fs_samples={samples:?} sample_count={}",
            samples.len()
        );
    }
}
