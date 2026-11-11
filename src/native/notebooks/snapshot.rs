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
    albums::{parse_album_declaration, AlbumDeclaration},
    config::config_path,
    projects::{
        frontmatter_is_album, frontmatter_is_area, frontmatter_is_notebook,
        frontmatter_is_project, frontmatter_value, is_excluded_directory,
        is_markdown_file, parse_frontmatter,
    },
    records::schema::{RecordDiagnostic, RecordType},
    vault_links::NoteIndex,
};

#[derive(Debug, Clone)]
pub(crate) struct NoteFactsLite {
    pub(crate) relative: PathBuf,
    pub(crate) relative_slash: String,
    pub(crate) stem: String,
    pub(crate) kind: LiteKind,
    pub(crate) parent_raw: Option<String>,
    pub(crate) parent_line: Option<usize>,
    pub(crate) parent_count: usize,
    pub(crate) summary_raw: Option<String>,
    pub(crate) aliases_first: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiteKind {
    Area,
    Project { terminal: bool },
    Notebook,
    Album,
    Ref,
    RecordInstance,
    Other,
    Untyped,
}

/// One indexed note: its facts, the minimal frontmatter header the home
/// resolver parses, and the original bytes for notebooks, areas, and
/// projects.
#[derive(Debug, Clone)]
pub(crate) struct SnapshotFile {
    pub(crate) lite: NoteFactsLite,
    pub(crate) header: String,
    pub(crate) contents: Option<String>,
}

/// Everything one analysis pass reads, detached from the filesystem.
#[derive(Debug, Clone)]
pub(crate) struct VaultSnapshot {
    /// Every discovered included path, readable or not, for link indexing.
    pub(crate) discovered: BTreeSet<PathBuf>,
    pub(crate) files: BTreeMap<PathBuf, SnapshotFile>,
    pub(crate) record_ids: BTreeSet<String>,
    config_record_ids: BTreeSet<String>,
    #[allow(dead_code)] // Consumed by the album registry/report phases.
    pub(crate) config_record_types: Vec<RecordType>,
    #[allow(dead_code)] // Consumed by the album registry/report phases.
    pub(crate) config_diagnostic: Option<RecordDiagnostic>,
    /// Parsed album claims and schema findings are independent of filesystem
    /// errors, so a broken declaration never turns a readable vault partial.
    pub(crate) album_declarations: BTreeMap<PathBuf, AlbumDeclaration>,
    #[allow(dead_code)] // Consumed by album diagnostics and report adapters.
    pub(crate) album_schema_diagnostics: Vec<RecordDiagnostic>,
    /// Failures from discovery or reads. A snapshot with I/O errors is a
    /// partial view: writers must refuse to plan from it.
    pub(crate) io_errors: Vec<String>,
}

impl VaultSnapshot {
    /// One sorted vault walk plus one read per included note.
    pub(crate) fn load(bob_dir: &Path) -> Self {
        Self::load_with_config(bob_dir, &config_path())
    }

    /// Load a snapshot using the caller's effective config path. Album
    /// registry callers pass both paths explicitly so `-b` never changes
    /// the vault while silently selecting another host's records config.
    pub(crate) fn load_with_config(bob_dir: &Path, config_path: &Path) -> Self {
        let mut relative_paths: Vec<PathBuf> = Vec::new();
        let mut io_errors = Vec::new();
        collect_markdown(bob_dir, bob_dir, &mut relative_paths, &mut io_errors);
        relative_paths.sort();
        relative_paths.dedup();

        let (config_record_types, config_diagnostic) =
            match crate::native::config::records::load_records_config_from(
                config_path,
            ) {
                Ok(config) => (config.types, None),
                Err(diagnostic) => (Vec::new(), Some(diagnostic)),
            };
        let mut config_record_ids = config_record_types
            .iter()
            .map(|record_type| record_type.id.clone())
            .collect::<BTreeSet<_>>();
        // Semantic config failures can still leave a valid YAML map of record
        // declarations. Preserve those structural nouns for frontmatter
        // recognition even though no config kind is usable.
        if config_diagnostic.is_some()
            && let Ok(text) = fs::read_to_string(config_path)
        {
            config_record_ids.extend(recover_record_ids(&text));
        }

        let mut contents_by_path = BTreeMap::new();
        for relative in &relative_paths {
            match fs::read_to_string(bob_dir.join(relative)) {
                Ok(contents) => {
                    contents_by_path.insert(relative.clone(), contents);
                }
                Err(error) => io_errors.push(format!(
                    "{}: failed to read file: {error}",
                    display_slash(relative)
                )),
            }
        }
        let mut snapshot = Self {
            discovered: relative_paths.into_iter().collect(),
            files: BTreeMap::new(),
            record_ids: config_record_ids.clone(),
            config_record_ids,
            config_record_types,
            config_diagnostic,
            album_declarations: BTreeMap::new(),
            album_schema_diagnostics: Vec::new(),
            io_errors,
        };
        snapshot.rebuild_classification(contents_by_path);
        snapshot
    }

    /// This snapshot with staged `(vault-relative path, contents)` files
    /// added or replaced, as if they had already been written.
    pub(crate) fn with_overlay(
        &self,
        staged: impl IntoIterator<Item = (PathBuf, String)>,
    ) -> Self {
        let mut next = self.clone();
        let mut contents_by_path = next
            .files
            .iter()
            .filter_map(|(path, file)| {
                file.contents
                    .as_ref()
                    .map(|contents| (path.clone(), contents.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        for (relative, contents) in staged {
            next.discovered.insert(relative.clone());
            contents_by_path.insert(relative, contents);
        }
        // Headers for non-album files are sufficient for classification;
        // retain their prior body state while declarations are reparsed.
        for (relative, file) in &next.files {
            if file.contents.is_none() {
                contents_by_path
                    .entry(relative.clone())
                    .or_insert_with(|| file.header.clone());
            }
        }
        next.rebuild_classification(contents_by_path);
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

    fn rebuild_classification(
        &mut self,
        contents_by_path: BTreeMap<PathBuf, String>,
    ) {
        let mut declarations = BTreeMap::new();
        let mut schema_diagnostics = Vec::new();
        for (path, contents) in &contents_by_path {
            if let Some(declaration) = parse_album_declaration(path, contents) {
                if let Some(diagnostic) = declaration.schema_diagnostic.clone()
                {
                    schema_diagnostics.push(diagnostic);
                }
                declarations.insert(path.clone(), declaration);
            }
        }
        self.record_ids = self.config_record_ids.clone();
        self.record_ids.extend(
            declarations
                .values()
                .filter_map(|declaration| declaration.noun_claim.clone()),
        );
        self.album_declarations = declarations;
        self.album_schema_diagnostics = schema_diagnostics;

        // Preserve unreadable paths in `discovered`; every readable file is
        // reconstructed from its header or full retained body with the new
        // album claims. This also removes stale classifications on overlays.
        let mut files = BTreeMap::new();
        for relative in &self.discovered {
            let Some(contents) = contents_by_path.get(relative) else {
                continue;
            };
            files.insert(
                relative.clone(),
                SnapshotFile::from_contents(
                    relative,
                    contents.clone(),
                    &self.record_ids,
                ),
            );
        }
        self.files = files;
    }
}

fn recover_record_ids(text: &str) -> BTreeSet<String> {
    let Ok(serde_yaml::Value::Mapping(root)) =
        serde_yaml::from_str::<serde_yaml::Value>(text)
    else {
        return BTreeSet::new();
    };
    let records_key = serde_yaml::Value::String("records".to_owned());
    let Some(serde_yaml::Value::Mapping(records)) = root.get(&records_key)
    else {
        return BTreeSet::new();
    };
    records
        .keys()
        .filter_map(serde_yaml::Value::as_str)
        .map(str::to_owned)
        .collect()
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
        let header = reconstruct_header(&frontmatter);
        let retain = matches!(
            kind,
            LiteKind::Notebook
                | LiteKind::Album
                | LiteKind::Area
                | LiteKind::Project { .. }
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

pub(crate) fn display_slash(path: &Path) -> String {
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
    if frontmatter_is_album(frontmatter) {
        return LiteKind::Album;
    }
    if frontmatter_is_notebook(frontmatter) {
        return LiteKind::Notebook;
    }
    let names = crate::native::projects::frontmatter_type_names(frontmatter);
    if names.iter().any(|name| name == "ref") {
        return LiteKind::Ref;
    }
    if crate::native::projects::frontmatter_is_record(frontmatter)
        || names.iter().any(|name| record_ids.contains(name))
    {
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
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some(value) = line.strip_prefix("parent:") else {
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

pub(crate) fn first_alias(contents: &str) -> Option<String> {
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
