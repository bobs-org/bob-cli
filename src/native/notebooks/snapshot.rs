//! Disk loading and in-memory overlays for the notebook scanner.
//!
//! One sorted discovery pass indexes every included note, reads its
//! frontmatter facts, and retains original bytes for notebooks, areas, and
//! projects (the only notes catalogs read or rewrite). Analysis then runs on
//! the snapshot alone, so a caller can overlay staged files (a new notebook,
//! a changed host) and plan catalogs without touching the filesystem.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use crate::native::{
    projects::{
        frontmatter_is_area, frontmatter_is_notebook, frontmatter_is_project,
        frontmatter_value, is_excluded_directory, is_markdown_file,
        parse_frontmatter,
    },
    vault_links::NoteIndex,
};

#[derive(Debug, Clone)]
pub(super) struct NoteFactsLite {
    pub(super) relative: PathBuf,
    pub(super) relative_slash: String,
    pub(super) stem: String,
    pub(super) kind: LiteKind,
    pub(super) parent_raw: Option<String>,
    pub(super) parent_line: Option<usize>,
    pub(super) parent_count: usize,
    pub(super) summary_raw: Option<String>,
    pub(super) aliases_first: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiteKind {
    Area,
    Project { terminal: bool },
    Notebook,
    Ref,
    RecordInstance,
    Other,
    Untyped,
}

/// One indexed note: its facts, the minimal frontmatter header the home
/// resolver parses, and the original bytes for notebooks, areas, and
/// projects.
#[derive(Debug, Clone)]
pub(super) struct SnapshotFile {
    pub(super) lite: NoteFactsLite,
    pub(super) header: String,
    pub(super) contents: Option<String>,
}

/// Everything one analysis pass reads, detached from the filesystem.
#[derive(Debug, Clone)]
pub(crate) struct VaultSnapshot {
    /// Every discovered included path, readable or not, for link indexing.
    pub(super) discovered: BTreeSet<PathBuf>,
    pub(super) files: BTreeMap<PathBuf, SnapshotFile>,
    pub(super) record_ids: BTreeSet<String>,
    /// Failures from discovery or reads. A snapshot with I/O errors is a
    /// partial view: writers must refuse to plan from it.
    pub(crate) io_errors: Vec<String>,
}

impl VaultSnapshot {
    /// One sorted vault walk plus one read per included note.
    pub(crate) fn load(bob_dir: &Path) -> Self {
        let mut relative_paths: Vec<PathBuf> = Vec::new();
        let mut io_errors = Vec::new();
        collect_markdown(bob_dir, bob_dir, &mut relative_paths, &mut io_errors);
        relative_paths.sort();
        relative_paths.dedup();

        let record_ids: BTreeSet<String> =
            match crate::native::config::records::load_records_config() {
                Ok(config) => config.types.into_iter().map(|t| t.id).collect(),
                Err(_) => BTreeSet::new(),
            };

        let mut files = BTreeMap::new();
        for relative in &relative_paths {
            match fs::read_to_string(bob_dir.join(relative)) {
                Ok(contents) => {
                    files.insert(
                        relative.clone(),
                        SnapshotFile::from_contents(
                            relative,
                            contents,
                            &record_ids,
                        ),
                    );
                }
                Err(error) => io_errors.push(format!(
                    "{}: failed to read file: {error}",
                    display_slash(relative)
                )),
            }
        }
        Self {
            discovered: relative_paths.into_iter().collect(),
            files,
            record_ids,
            io_errors,
        }
    }

    /// This snapshot with staged `(vault-relative path, contents)` files
    /// added or replaced, as if they had already been written.
    pub(crate) fn with_overlay(
        &self,
        staged: impl IntoIterator<Item = (PathBuf, String)>,
    ) -> Self {
        let mut next = self.clone();
        for (relative, contents) in staged {
            next.discovered.insert(relative.clone());
            let file = SnapshotFile::from_contents(
                &relative,
                contents,
                &next.record_ids,
            );
            next.files.insert(relative, file);
        }
        next
    }

    pub(crate) fn index(&self) -> NoteIndex {
        NoteIndex::from_paths(self.discovered.iter().cloned())
    }

    /// Original bytes for a retained note (notebook, area, or project).
    pub(crate) fn contents(&self, relative: &Path) -> Option<&str> {
        self.files
            .get(relative)
            .and_then(|file| file.contents.as_deref())
    }

    /// Whether any discovered path equals `relative` ignoring ASCII case.
    pub(crate) fn contains_ignore_case(&self, relative: &Path) -> bool {
        let wanted = display_slash(relative).to_lowercase();
        self.discovered
            .iter()
            .any(|path| display_slash(path).to_lowercase() == wanted)
    }
}

impl SnapshotFile {
    pub(super) fn from_contents(
        relative: &Path,
        contents: String,
        record_ids: &BTreeSet<String>,
    ) -> Self {
        let slash = display_slash(relative);
        let stem = stem_of(relative);
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            // No frontmatter: an untyped note without facts, still indexed.
            return Self {
                lite: NoteFactsLite {
                    relative: relative.to_path_buf(),
                    relative_slash: slash,
                    stem,
                    kind: LiteKind::Untyped,
                    parent_raw: None,
                    parent_line: None,
                    parent_count: 0,
                    summary_raw: None,
                    aliases_first: None,
                },
                header: "---\n---\n".to_string(),
                contents: None,
            };
        };
        let kind = classify_kind(&frontmatter, record_ids);
        let (parent_raw, parent_line, parent_count) = parent_info(&frontmatter);
        let summary_raw =
            frontmatter_value(&frontmatter, "summary").map(str::to_string);
        let aliases_first = first_alias(&contents);
        let is_notebook = frontmatter_is_notebook(&frontmatter);
        let header = reconstruct_header(&frontmatter);
        let kind = if is_notebook {
            LiteKind::Notebook
        } else {
            kind
        };
        let retain = matches!(
            kind,
            LiteKind::Notebook | LiteKind::Area | LiteKind::Project { .. }
        );
        Self {
            lite: NoteFactsLite {
                relative: relative.to_path_buf(),
                relative_slash: slash,
                stem,
                kind,
                parent_raw,
                parent_line,
                parent_count,
                summary_raw,
                aliases_first,
            },
            header,
            contents: retain.then_some(contents),
        }
    }
}

pub(super) fn collect_markdown(
    root: &Path,
    directory: &Path,
    out: &mut Vec<PathBuf>,
    io_errors: &mut Vec<String>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
        Err(error) => {
            io_errors.push(format!(
                "{}: failed to read directory: {error}",
                display_slash(&relative_or(root, directory))
            ));
            return;
        }
    };
    let Ok(mut entries) = entries else {
        io_errors.push(format!(
            "{}: failed to read directory entries",
            display_slash(&relative_or(root, directory))
        ));
        return;
    };
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                io_errors.push(format!(
                    "{}: failed to inspect path: {error}",
                    display_slash(&relative_or(root, &path))
                ));
                continue;
            }
        };
        if file_type.is_dir() {
            if is_excluded_directory(&path)
                || crate::native::done_archive::skip_archive_directory(
                    root, &path,
                )
            {
                continue;
            }
            // `_templates/` is already excluded via ALWAYS_EXCLUDED, but
            // keep the explicit guard for the notebook contract.
            if path.file_name().and_then(|name| name.to_str())
                == Some("_templates")
            {
                continue;
            }
            collect_markdown(root, &path, out, io_errors);
            continue;
        }
        if file_type.is_file() && is_markdown_file(&path) {
            out.push(relative_or(root, &path));
        }
    }
}

pub(super) fn relative_or(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

pub(super) fn display_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(super) fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_string()
}

pub(super) fn stem_of_path_str(value: &str) -> String {
    let trimmed = value.trim();
    let without_suffix = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".MD"))
        .unwrap_or(trimmed);
    Path::new(without_suffix)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| without_suffix.to_string())
}

pub(super) fn classify_kind(
    frontmatter: &crate::native::projects::Frontmatter<'_>,
    record_ids: &BTreeSet<String>,
) -> LiteKind {
    if frontmatter_is_area(frontmatter) {
        return LiteKind::Area;
    }
    if frontmatter_is_project(frontmatter) {
        let status = crate::native::projects::ProjectStatus::parse(
            frontmatter_value(frontmatter, "status"),
        );
        return LiteKind::Project {
            terminal: status.is_terminal(),
        };
    }
    if frontmatter_is_notebook(frontmatter) {
        return LiteKind::Notebook;
    }
    let names = crate::native::projects::frontmatter_type_names(frontmatter);
    if names.iter().any(|name| name == "ref") {
        return LiteKind::Ref;
    }
    if names.iter().any(|name| record_ids.contains(name)) {
        return LiteKind::RecordInstance;
    }
    if names.is_empty() {
        return LiteKind::Untyped;
    }
    LiteKind::Other
}

pub(super) fn parent_info(
    frontmatter: &crate::native::projects::Frontmatter<'_>,
) -> (Option<String>, Option<usize>, usize) {
    let mut count = 0;
    let mut first_raw = None;
    let mut first_line = None;
    for (index, line) in frontmatter.lines.iter().enumerate() {
        // Frontmatter lines are 2-based (line 1 is `---`).
        let number = index + 2;
        let Some(rest) = line.strip_prefix("parent") else {
            continue;
        };
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        count += 1;
        if first_raw.is_none() {
            first_raw = Some(value.trim().to_string());
            first_line = Some(number);
        }
    }
    (
        first_raw.filter(|value| !value.is_empty()),
        first_line,
        count,
    )
}

pub(super) fn reconstruct_header(
    frontmatter: &crate::native::projects::Frontmatter<'_>,
) -> String {
    let mut header = String::from("---\n");
    for line in &frontmatter.lines {
        header.push_str(line);
        header.push('\n');
    }
    header.push_str("---\n");
    header
}

pub(super) fn first_alias(contents: &str) -> Option<String> {
    let start = contents.find("---")?;
    let rest = &contents[start + 3..];
    let end = rest.find("---")?;
    let block = &rest[..end];
    let parsed: serde_yaml::Value = serde_yaml::from_str(block).ok()?;
    let mapping = parsed.as_mapping()?;
    let key = serde_yaml::Value::String("aliases".to_string());
    let value = mapping.get(&key)?;
    match value {
        serde_yaml::Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        serde_yaml::Value::Sequence(items) => items.iter().find_map(|item| {
            item.as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        }),
        _ => None,
    }
}
