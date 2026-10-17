//! Vault membership, note-kind classification, and cached source reads.
//!
//! One [`SourceStore`] lives per projection operation. The root note, embedded
//! notes, and notes whose block labels are inspected all read through it, so
//! every consumer shares the same immutable bytes and the root SHA-256 comes
//! from those exact bytes. Read errors are cached too, and path discovery
//! never entitles eager reads: bytes are loaded on demand only.
//!
//! [`classify_target`] preserves today's filesystem-target precedence: an
//! existing non-vault `.md` or PDF file keeps its plain route, and an excluded
//! existing Markdown path remains ordinary Markdown. The default-parent walk
//! is explicitly deferred to outer create-wiring (see [`default_parent`]);
//! it is documented at this API rather than left as a misleading stub.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fmt, fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
use std::cell::Cell;

use sha2::{Digest, Sha256};

use super::resolve::{parse_link_target, VaultNotes};
use super::view::SourceIdentity;
use crate::native::{
    is_always_excluded_note_directory_name,
    projects::{
        frontmatter_is_area, frontmatter_is_project, frontmatter_value,
        parse_frontmatter, ProjectStatus,
    },
};

/// How a snapshot input was classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteKind {
    /// A vault note without area/project frontmatter.
    Ordinary,
    /// Frontmatter `type` is `[[area]]`.
    Area,
    /// Frontmatter `type` is `[[project]]` with a non-terminal status.
    Project,
    /// Frontmatter `type` is `[[project]]` with a terminal (`done`,
    /// `canceled`) status. The Bob layer still needs the raw frontmatter
    /// for its own status handling; this variant only records eligibility.
    TerminalProject,
}

impl NoteKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "note",
            Self::Area => "area",
            Self::Project => "project",
            Self::TerminalProject => "terminal-project",
        }
    }
}

/// A vault note accepted for snapshotting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifiedNote {
    /// The target path exactly as passed, for caller diagnostics.
    pub display_path: String,
    /// Canonical filesystem identity of the note.
    pub canonical_path: PathBuf,
    /// Vault identity (canonical path, vault-relative path, route).
    pub identity: SourceIdentity,
    /// Kind read from cached frontmatter, never from path heuristics.
    pub kind: NoteKind,
}

/// Every `classify_target` outcome. `VaultNote` is the only snapshot route;
/// the remaining variants preserve today's plain workflows untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SnapshotTarget {
    /// A member vault note, ready for the snapshot pipeline.
    VaultNote(ClassifiedNote),
    /// An existing Markdown file outside the vault (or in an excluded
    /// directory): today's plain-Markdown rendering keeps it.
    PlainMarkdown(PathBuf),
    /// An existing PDF file: today's stamp/install route keeps it.
    Pdf(PathBuf),
    /// An existing file that is neither Markdown nor PDF: create-wiring
    /// reports today's `TARGET must be ...` error for it.
    OtherExistingFile(PathBuf),
}

/// Typed `classify_target` failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClassifyError {
    /// No file or vault note answers the target, with up to three near
    /// stems when any qualify.
    Missing {
        display: String,
        suggestions: Vec<String>,
    },
    /// Several member notes share the target's basename, with the sorted
    /// complete candidate list.
    Ambiguous {
        display: String,
        candidates: Vec<String>,
    },
}

impl fmt::Display for ClassifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing {
                display,
                suggestions,
            } => {
                write!(formatter, "no file or vault note named '{display}'")?;
                if !suggestions.is_empty() {
                    let quoted = suggestions
                        .iter()
                        .map(|stem| format!("'{stem}'"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(formatter, "\nhint: did you mean {quoted}?")?;
                }
                Ok(())
            }
            Self::Ambiguous {
                display,
                candidates,
            } => {
                write!(formatter, "multiple vault notes named '{display}':")?;
                for candidate in candidates {
                    write!(formatter, "\n  {candidate}")?;
                }
                write!(
                    formatter,
                    "\nhint: pass a vault-relative path instead of a bare name"
                )
            }
        }
    }
}

impl std::error::Error for ClassifyError {}

/// Reads note bytes. The test seam is [`CountingLoader`]; production uses
/// [`FilesystemSourceLoader`].
pub(crate) trait SourceLoader {
    fn load(&self, path: &Path) -> io::Result<Vec<u8>>;
}

/// Production [`SourceLoader`]: straight filesystem reads.
#[derive(Debug, Default)]
pub(crate) struct FilesystemSourceLoader;

impl SourceLoader for FilesystemSourceLoader {
    fn load(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }
}

/// Counting [`SourceLoader`] for tests: serves fixed bytes and records how
/// many backing reads happened, proving the store reads each source once.
#[cfg(test)]
pub(crate) struct CountingLoader {
    files: HashMap<PathBuf, Vec<u8>>,
    reads: Cell<usize>,
}

#[cfg(test)]
impl CountingLoader {
    pub(crate) fn with_files(files: Vec<(PathBuf, Vec<u8>)>) -> Self {
        Self {
            files: files.into_iter().collect(),
            reads: Cell::new(0),
        }
    }

    pub(crate) fn reads(&self) -> usize {
        self.reads.get()
    }
}

#[cfg(test)]
impl SourceLoader for CountingLoader {
    fn load(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.reads.set(self.reads.get() + 1);
        self.files.get(path).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("synthetic source not found: {}", path.display()),
            )
        })
    }
}

/// Cached, canonical-path-keyed source bytes for one projection operation.
pub(crate) struct SourceStore<L: SourceLoader = FilesystemSourceLoader> {
    vault_root: PathBuf,
    vault_name: String,
    loader: L,
    cache: HashMap<PathBuf, Result<Arc<[u8]>, String>>,
}

impl<L: SourceLoader> SourceStore<L> {
    /// Canonicalize `vault_root` once and derive the vault display name used
    /// by the shared `obsidian://` builder.
    pub(crate) fn with_loader(
        vault_root: &Path,
        loader: L,
    ) -> io::Result<Self> {
        let vault_root = vault_root.canonicalize()?;
        let vault_name = vault_root
            .file_name()
            .and_then(OsStr::to_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "vault directory has no UTF-8 basename: {}",
                        vault_root.display()
                    ),
                )
            })?
            .to_string();
        Ok(Self {
            vault_root,
            vault_name,
            loader,
            cache: HashMap::new(),
        })
    }

    pub(crate) fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    pub(crate) fn vault_name(&self) -> &str {
        &self.vault_name
    }

    pub(crate) fn loader(&self) -> &L {
        &self.loader
    }

    /// Note helpers over this store's vault root.
    pub(crate) fn vault_notes(&self) -> VaultNotes {
        VaultNotes::new(self.vault_root.clone(), self.vault_name.clone())
    }

    /// Read one canonical path, caching bytes and errors alike. Repeated
    /// reads return the same immutable allocation.
    pub(crate) fn read_canonical(
        &mut self,
        canonical: &Path,
    ) -> Result<Arc<[u8]>, String> {
        if let Some(hit) = self.cache.get(canonical) {
            return hit.clone();
        }
        let result = self
            .loader
            .load(canonical)
            .map(Vec::into_boxed_slice)
            .map(Arc::from)
            .map_err(|error| format!("read {}: {error}", canonical.display()));
        self.cache.insert(canonical.to_path_buf(), result.clone());
        result
    }

    /// Resolve a vault-relative path to its canonical location (rejecting
    /// symlink escapes) and read it through the cache. Canonicalization
    /// failures are cached under the joined path so retries stay cheap.
    pub(crate) fn read_vault_relative(
        &mut self,
        relative: &Path,
    ) -> Result<(PathBuf, Arc<[u8]>), String> {
        let joined = self.vault_root.join(relative);
        let canonical = match joined.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) => {
                let message = format!("read {}: {error}", joined.display());
                self.cache
                    .entry(joined)
                    .or_insert_with(|| Err(message.clone()));
                return Err(message);
            }
        };
        if canonical != self.vault_root
            && !is_within_vault(&self.vault_root, &canonical)
        {
            let message = format!(
                "vault path {} escapes the vault {}",
                canonical.display(),
                self.vault_root.display()
            );
            self.cache
                .entry(joined)
                .or_insert_with(|| Err(message.clone()));
            return Err(message);
        }
        let bytes = self.read_canonical(&canonical)?;
        self.cache
            .entry(joined)
            .or_insert_with(|| Ok(bytes.clone()));
        Ok((canonical, bytes))
    }
}

impl SourceStore<FilesystemSourceLoader> {
    /// Open a store backed by real filesystem reads.
    pub(crate) fn open(vault_root: &Path) -> io::Result<Self> {
        Self::with_loader(vault_root, FilesystemSourceLoader)
    }
}

/// Hex SHA-256 of source bytes. Callers hash the exact cached bytes so the
/// root digest, excerpts, and transclusion content stay consistent even if
/// the file changes mid-projection.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Vault identity for a canonical member path: `/`-joined vault-relative
/// path plus the Obsidian route (relative path without its `.md` suffix).
pub(crate) fn source_identity(
    vault_root: &Path,
    canonical: &Path,
) -> Option<SourceIdentity> {
    let relative = canonical.strip_prefix(vault_root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let vault_relative_path = relative.to_string_lossy().replace('\\', "/");
    let route = vault_relative_path
        .strip_suffix(".md")
        .or_else(|| vault_relative_path.strip_suffix(".MD"))
        .unwrap_or(&vault_relative_path)
        .to_string();
    Some(SourceIdentity {
        canonical_path: canonical.to_path_buf(),
        vault_relative_path,
        route,
    })
}

/// True for directory components that disqualify vault membership: every
/// dot-directory plus `_conflicts`, `xlib`, `lib`, `lit_review`, and the
/// shared always-excluded set. Non-UTF-8 components are excluded as the
/// safe default.
pub(crate) fn is_excluded_note_dir_component(name: &OsStr) -> bool {
    let Some(name_str) = name.to_str() else {
        return true;
    };
    name_str.starts_with('.')
        || matches!(name_str, "_conflicts" | "xlib" | "lib" | "lit_review")
        || is_always_excluded_note_directory_name(name)
}

/// Strip `vault_root` from an already-canonicalized candidate and enforce
/// every directory exclusion. Returns the vault-relative path on success.
pub(crate) fn vault_member_relative(
    vault_root: &Path,
    candidate_canonical: &Path,
) -> Option<PathBuf> {
    let relative = candidate_canonical.strip_prefix(vault_root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => names.push(name),
            _ => return None,
        }
    }
    // Every directory component must clear the exclusion list; the file
    // name itself is never excluded.
    let directories = names.get(..names.len().saturating_sub(1))?;
    for directory in directories {
        if is_excluded_note_dir_component(directory) {
            return None;
        }
    }
    Some(relative.to_path_buf())
}

/// True when any directory component of a vault-relative path is a
/// dot-directory. The file name itself is ignored, matching
/// [`vault_member_relative`].
pub(crate) fn relative_has_dot_directory(relative: &Path) -> bool {
    let mut names = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => names.push(name),
            _ => return true,
        }
    }
    let Some(directories) = names.get(..names.len().saturating_sub(1)) else {
        return false;
    };
    directories
        .iter()
        .any(|name| name.to_str().is_some_and(|name| name.starts_with('.')))
}

fn is_within_vault(vault_root: &Path, canonical: &Path) -> bool {
    canonical.starts_with(vault_root)
}

/// Classify one note's kind from its cached source text. Frontmatter is the
/// only signal; path and name heuristics never decide.
pub(crate) fn classify_note_kind(source_text: &str) -> NoteKind {
    let Some(frontmatter) = parse_frontmatter(source_text) else {
        return NoteKind::Ordinary;
    };
    if frontmatter_is_area(&frontmatter) {
        return NoteKind::Area;
    }
    if frontmatter_is_project(&frontmatter) {
        let status =
            ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
        if status.is_terminal() {
            return NoteKind::TerminalProject;
        }
        return NoteKind::Project;
    }
    NoteKind::Ordinary
}

/// True when the file starts with the PDF magic anywhere in its first bytes,
/// mirroring today's `create` PDF sniffing.
fn file_starts_with_pdf(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let head = &bytes[..bytes.len().min(1024)];
    head.windows(5).any(|window| window == b"%PDF-")
}

fn markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn pdf_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

/// Classify one `create` TARGET. Existing `.md`/PDF files keep today's plain
/// routes (an excluded existing Markdown path stays ordinary Markdown);
/// anything else resolves as a vault note reference (`x`, `x.md`, `dir/x`,
/// or a `[[x]]`/`[[x|alias]]` wrapper, whose alias and fragment are ignored
/// for whole-note classification).
pub(crate) fn classify_target<L: SourceLoader>(
    store: &mut SourceStore<L>,
    raw_target: &str,
) -> Result<SnapshotTarget, ClassifyError> {
    let trimmed = raw_target.trim();
    let vault = store.vault_notes();

    // Existing filesystem paths keep today's precedence.
    let path = Path::new(trimmed);
    if path.is_file() {
        let canonical =
            path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if markdown_extension(path) {
            if vault_member_relative(store.vault_root(), &canonical).is_some() {
                return Ok(SnapshotTarget::VaultNote(read_member_note(
                    store, trimmed, &canonical,
                )?));
            }
            return Ok(SnapshotTarget::PlainMarkdown(path.to_path_buf()));
        }
        if pdf_extension(path) || file_starts_with_pdf(path) {
            return Ok(SnapshotTarget::Pdf(path.to_path_buf()));
        }
        return Ok(SnapshotTarget::OtherExistingFile(path.to_path_buf()));
    }

    // Otherwise the target is a vault note reference.
    let parsed = super::resolve::parse_link_target(trimmed);
    if parsed.note.is_empty() {
        return Err(ClassifyError::Missing {
            display: trimmed.to_string(),
            suggestions: Vec::new(),
        });
    }
    match vault.resolve_note(Path::new(""), &parsed) {
        Ok(resolved) => Ok(SnapshotTarget::VaultNote(read_member_note(
            store,
            trimmed,
            &resolved.canonical_path,
        )?)),
        Err(error) => {
            // An explicitly named excluded file keeps the ordinary Markdown
            // route instead of failing as a vault note. Bare basenames that
            // only match excluded notes stay missing: only an exact
            // vault-relative reference detours here, so excluded copies can
            // never shadow (or answer for) real notes.
            if let Some(canonical) = vault.existing_vault_file(&parsed) {
                if vault_member_relative(store.vault_root(), &canonical)
                    .is_none()
                    && markdown_extension(&canonical)
                    && is_explicit_vault_path(
                        store.vault_root(),
                        &canonical,
                        &parsed.note,
                    )
                {
                    return Ok(SnapshotTarget::PlainMarkdown(canonical));
                }
            }
            Err(super::resolve::classify_error_from_resolve(trimmed, error))
        }
    }
}

/// True when the reference names the vault file exactly (with or without
/// its `.md` suffix), rather than reaching it through basename fallback.
fn is_explicit_vault_path(
    vault_root: &Path,
    canonical: &Path,
    note_part: &str,
) -> bool {
    let Ok(relative) = canonical.strip_prefix(vault_root) else {
        return false;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    let without_suffix = relative
        .strip_suffix(".md")
        .or_else(|| relative.strip_suffix(".MD"))
        .unwrap_or(&relative);
    note_part == relative || note_part == without_suffix
}

fn read_member_note<L: SourceLoader>(
    store: &mut SourceStore<L>,
    display_path: &str,
    canonical: &Path,
) -> Result<ClassifiedNote, ClassifyError> {
    let bytes = store.read_canonical(canonical).map_err(|_| {
        ClassifyError::Missing {
            display: display_path.to_string(),
            suggestions: Vec::new(),
        }
    })?;
    let text = String::from_utf8_lossy(&bytes);
    let kind = classify_note_kind(&text);
    let identity = source_identity(store.vault_root(), canonical)
        .expect("classified member notes stay within the vault");
    Ok(ClassifiedNote {
        display_path: display_path.to_string(),
        canonical_path: canonical.to_path_buf(),
        identity,
        kind,
    })
}

/// The inferred parent and why it was selected, for dry-run reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultParent {
    pub resolved: crate::native::parent_notes::ResolvedParent,
    /// `the note itself` or `ancestor <route>`.
    pub reason: String,
}

/// Find the nearest eligible root area/project in a note's parent chain.
/// The walk is cycle-safe, bounded to eight ancestor hops, and reads every
/// note through the same cache used by projection.
pub(crate) fn default_parent<L: SourceLoader>(
    store: &mut SourceStore<L>,
    note: &ClassifiedNote,
) -> Option<DefaultParent> {
    const MAX_ANCESTORS: usize = 8;

    let root_bytes = store.read_canonical(&note.canonical_path).ok()?;
    let mut current_identity = note.identity.clone();
    let mut current_text = String::from_utf8_lossy(&root_bytes).into_owned();
    if let Some(resolved) =
        crate::native::parent_notes::resolve_cached_note_parent(
            &current_identity.route,
            &current_text,
        )
    {
        return Some(DefaultParent {
            resolved,
            reason: "the note itself".to_string(),
        });
    }

    let vault = store.vault_notes();
    let mut visited = HashSet::from([current_identity.canonical_path.clone()]);
    for _ in 0..MAX_ANCESTORS {
        let parent = cached_parent_link(&current_text)?;
        let parsed = parse_link_target(&parent);
        let resolved = vault
            .resolve_note(
                Path::new(&current_identity.vault_relative_path),
                &parsed,
            )
            .ok()?;
        if !visited.insert(resolved.canonical_path.clone()) {
            return None;
        }
        let bytes = store.read_canonical(&resolved.canonical_path).ok()?;
        current_text = String::from_utf8_lossy(&bytes).into_owned();
        current_identity =
            source_identity(store.vault_root(), &resolved.canonical_path)?;
        if let Some(parent) =
            crate::native::parent_notes::resolve_cached_note_parent(
                &current_identity.route,
                &current_text,
            )
        {
            return Some(DefaultParent {
                reason: format!("ancestor {}", parent.route),
                resolved: parent,
            });
        }
    }
    None
}

fn cached_parent_link(contents: &str) -> Option<String> {
    let frontmatter = parse_frontmatter(contents)?;
    let value = frontmatter_value(&frontmatter, "parent")?;
    let value = crate::native::projects::trim_yaml_scalar(value).trim();
    if value.is_empty() || value.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A synthetic vault on disk: real files for the resolver's exact-hit
    /// checks, with bytes optionally preloaded into a counting loader so
    /// the store never touches disk.
    struct SyntheticVault {
        _directory: tempfile::TempDir,
        root: PathBuf,
    }

    impl SyntheticVault {
        fn new(files: &[(&str, &str)]) -> Self {
            let directory = tempfile::tempdir().expect("temp vault");
            for (relative, contents) in files {
                let path = directory.path().join(relative);
                fs::create_dir_all(path.parent().expect("parent"))
                    .expect("create parent");
                fs::write(&path, contents).expect("write note");
            }
            let root = directory.path().canonicalize().expect("canonical");
            Self {
                _directory: directory,
                root,
            }
        }

        fn counting_store(&self) -> SourceStore<CountingLoader> {
            let mut preloaded = Vec::new();
            collect_files(&self.root, &mut preloaded);
            let files = preloaded
                .iter()
                .map(|path| {
                    (path.clone(), fs::read(path).expect("read synthetic note"))
                })
                .collect();
            SourceStore::with_loader(
                &self.root,
                CountingLoader::with_files(files),
            )
            .expect("open store")
        }

        fn filesystem_store(&self) -> SourceStore<FilesystemSourceLoader> {
            SourceStore::open(&self.root).expect("open store")
        }
    }

    fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("read dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if entry.file_type().expect("type").is_dir() {
                collect_files(&path, files);
            } else {
                files.push(path.canonicalize().expect("canonical file"));
            }
        }
    }

    const ORDINARY: &str = "# Plain note\n\nJust prose.\n";
    const AREA: &str = "---\ntype: \"[[area]]\"\n---\n# Area\n";
    const PROJECT: &str =
        "---\ntype: [[project]]\nstatus: wip\n---\n# Project\n";
    const DONE_PROJECT: &str =
        "---\ntype: [[project]]\nstatus: done\n---\n# Done\n";
    const CANCELED_PROJECT: &str =
        "---\ntype: '[[project]]'\nstatus: canceled\n---\n# Gone\n";

    #[test]
    fn kind_comes_from_cached_frontmatter_not_paths() {
        for (relative, expected) in [
            ("plain.md", NoteKind::Ordinary),
            ("Area Note.md", NoteKind::Area),
            ("projects/Ship.md", NoteKind::Project),
            ("projects/Done.md", NoteKind::TerminalProject),
            ("projects/Gone.md", NoteKind::TerminalProject),
            ("zk/subfolder/Sub Idea.md", NoteKind::Project),
        ] {
            let vault = SyntheticVault::new(&[(
                relative,
                match expected {
                    NoteKind::Ordinary => ORDINARY,
                    NoteKind::Area => AREA,
                    NoteKind::Project => PROJECT,
                    NoteKind::TerminalProject => {
                        if relative.contains("Gone") {
                            CANCELED_PROJECT
                        } else {
                            DONE_PROJECT
                        }
                    }
                },
            )]);
            let mut store = vault.counting_store();
            let stem = Path::new(relative)
                .file_stem()
                .expect("stem")
                .to_string_lossy()
                .into_owned();
            let target = classify_target(&mut store, &stem).expect("classify");
            let SnapshotTarget::VaultNote(note) = target else {
                panic!("expected vault note for {relative}");
            };
            assert_eq!(note.kind, expected, "{relative}");
            if relative.contains('/') {
                assert!(
                    note.identity.route.contains('/'),
                    "subfolder route keeps its directory: {}",
                    note.identity.route
                );
            }
        }
    }

    #[test]
    fn existing_files_keep_their_plain_routes() {
        let outside = tempfile::tempdir().expect("outside dir");
        let plain = outside.path().join("loose.md");
        let pdf = outside.path().join("paper.pdf");
        let pdf_magic = outside.path().join("no-extension");
        fs::write(&plain, "# loose\n").expect("write");
        fs::write(&pdf, "%PDF-1.7 fake\n").expect("write");
        fs::write(&pdf_magic, "%PDF-1.4 fake\n").expect("write");

        let vault = SyntheticVault::new(&[("Note.md", ORDINARY)]);
        let mut store = vault.filesystem_store();
        assert_eq!(
            classify_target(&mut store, &plain.to_string_lossy())
                .expect("classify"),
            SnapshotTarget::PlainMarkdown(plain.clone())
        );
        assert_eq!(
            classify_target(&mut store, &pdf.to_string_lossy())
                .expect("classify"),
            SnapshotTarget::Pdf(pdf.clone())
        );
        assert_eq!(
            classify_target(&mut store, &pdf_magic.to_string_lossy())
                .expect("classify"),
            SnapshotTarget::Pdf(pdf_magic.clone())
        );
    }

    #[test]
    fn excluded_existing_markdown_stays_ordinary() {
        let excluded = [
            ".hidden/Secret.md",
            ".obsidian/Config.md",
            "_conflicts/Clash.md",
            "_generated/Auto.md",
            "_templates/Today.md",
            "xlib/Intake.md",
            "lib/Stored.md",
            "lit_review/Old.md",
        ];
        let files = excluded
            .iter()
            .map(|relative| (*relative, ORDINARY))
            .collect::<Vec<_>>();
        let vault = SyntheticVault::new(&files);
        let mut store = vault.filesystem_store();
        for relative in excluded {
            let absolute = vault.root.join(relative);
            let target =
                classify_target(&mut store, &absolute.to_string_lossy())
                    .expect("classify");
            assert_eq!(
                target,
                SnapshotTarget::PlainMarkdown(absolute.clone()),
                "{relative}"
            );
            // The basename form must not resolve into the excluded copy.
            let stem = Path::new(relative)
                .file_stem()
                .expect("stem")
                .to_string_lossy()
                .into_owned();
            let error =
                classify_target(&mut store, &stem).expect_err("missing");
            assert!(
                matches!(error, ClassifyError::Missing { .. }),
                "{stem}: {error}"
            );
            // An explicit vault-relative reference to the same file keeps
            // the ordinary Markdown route, exactly like its absolute path.
            let explicit = relative.strip_suffix(".md").expect("md suffix");
            let target = classify_target(&mut store, explicit)
                .expect("explicit excluded");
            assert_eq!(
                target,
                SnapshotTarget::PlainMarkdown(absolute),
                "{explicit}"
            );
        }
    }

    #[test]
    fn exact_paths_win_over_mixed_case_basename_twins() {
        let vault = SyntheticVault::new(&[
            ("Projects/Alpha.md", PROJECT),
            ("Archive/alpha.md", PROJECT),
            ("Solo.md", ORDINARY),
        ]);
        let mut store = vault.filesystem_store();
        let target =
            classify_target(&mut store, "Projects/Alpha").expect("exact path");
        let SnapshotTarget::VaultNote(note) = target else {
            panic!("expected vault note");
        };
        assert_eq!(note.identity.vault_relative_path, "Projects/Alpha.md");

        let target =
            classify_target(&mut store, "solo").expect("basename fallback");
        let SnapshotTarget::VaultNote(note) = target else {
            panic!("expected vault note");
        };
        assert_eq!(note.identity.vault_relative_path, "Solo.md");

        let error = classify_target(&mut store, "Alpha").expect_err("dup");
        let ClassifyError::Ambiguous {
            display,
            candidates,
        } = error
        else {
            panic!("expected ambiguity");
        };
        assert_eq!(display, "Alpha");
        assert_eq!(
            candidates,
            vec![
                "Archive/alpha.md".to_string(),
                "Projects/Alpha.md".to_string()
            ]
        );
    }

    #[test]
    fn missing_names_suggest_up_to_three_near_stems() {
        let vault = SyntheticVault::new(&[
            ("sase_blog.md", ORDINARY),
            ("sase_blog_notes.md", ORDINARY),
            ("cash_flow.md", ORDINARY),
            ("unrelated.md", ORDINARY),
        ]);
        let mut store = vault.filesystem_store();
        let error = classify_target(&mut store, "sase_blg").expect_err("miss");
        let ClassifyError::Missing {
            display,
            suggestions,
        } = error
        else {
            panic!("expected missing");
        };
        assert_eq!(display, "sase_blg");
        assert!(
            suggestions.len() <= 3 && !suggestions.is_empty(),
            "suggestions: {suggestions:?}"
        );
        assert!(suggestions.contains(&"sase_blog".to_string()));
        assert!(!suggestions.contains(&"unrelated".to_string()));
        let rendered = ClassifyError::Missing {
            display: display.clone(),
            suggestions: suggestions.clone(),
        }
        .to_string();
        assert!(
            rendered.starts_with("no file or vault note named 'sase_blg'"),
            "{rendered}"
        );
    }

    #[test]
    fn wikilink_wrappers_aliases_and_fragments_classify_the_note() {
        let vault = SyntheticVault::new(&[("-diary/Deep Note.md", AREA)]);
        let mut store = vault.filesystem_store();
        for target in [
            "[[Deep Note]]",
            "[[Deep Note|alias]]",
            "[[Deep Note#Heading]]",
            "[[Deep Note#^block]]",
            "Deep Note.md",
        ] {
            let classified =
                classify_target(&mut store, target).expect("classify {target}");
            let SnapshotTarget::VaultNote(note) = classified else {
                panic!("expected vault note for {target}");
            };
            assert_eq!(note.kind, NoteKind::Area, "{target}");
        }
    }

    #[test]
    fn sources_read_once_and_share_immutable_bytes() {
        let vault = SyntheticVault::new(&[("Note.md", ORDINARY)]);
        let mut store = vault.counting_store();
        let first = classify_target(&mut store, "Note").expect("first");
        let second = classify_target(&mut store, "[[Note]]").expect("second");
        let (
            SnapshotTarget::VaultNote(first),
            SnapshotTarget::VaultNote(second),
        ) = (first, second)
        else {
            panic!("expected vault notes");
        };
        assert_eq!(first.canonical_path, second.canonical_path);
        assert_eq!(first.kind, second.kind);
        assert_eq!(store.loader().reads(), 1);

        let canonical = vault.root.join("Note.md");
        let one = store.read_canonical(&canonical).expect("read");
        let two = store.read_canonical(&canonical).expect("read");
        assert!(Arc::ptr_eq(&one, &two));
        assert_eq!(store.loader().reads(), 1);
        assert_eq!(
            sha256_hex(&one),
            hex::encode(sha2::Sha256::digest(ORDINARY.as_bytes()))
        );
    }

    #[test]
    fn read_errors_are_cached_too() {
        let vault = SyntheticVault::new(&[]);
        let mut store = vault.counting_store();
        let missing = vault.root.join("Gone.md");
        assert!(store.read_canonical(&missing).is_err());
        assert!(store.read_canonical(&missing).is_err());
        assert_eq!(store.loader().reads(), 1);
    }

    #[test]
    fn default_parent_prefers_self_then_walks_eligible_ancestors() {
        let vault = SyntheticVault::new(&[
            ("Area.md", AREA),
            (
                "projects/Done.md",
                "---\ntype: [[project]]\nstatus: done\nparent: [[Area]]\n---\n",
            ),
            (
                "zk/Idea.md",
                "---\nparent: [[projects/Done]]\n---\n# Idea\n",
            ),
        ]);
        let mut store = vault.filesystem_store();
        let SnapshotTarget::VaultNote(area) =
            classify_target(&mut store, "Area").expect("classify area")
        else {
            panic!("expected area note");
        };
        let self_parent =
            default_parent(&mut store, &area).expect("self default");
        assert_eq!(self_parent.resolved.route, "Area");
        assert_eq!(self_parent.reason, "the note itself");

        let SnapshotTarget::VaultNote(idea) =
            classify_target(&mut store, "zk/Idea").expect("classify idea")
        else {
            panic!("expected idea note");
        };
        let ancestor =
            default_parent(&mut store, &idea).expect("ancestor default");
        assert_eq!(ancestor.resolved.route, "Area");
        assert_eq!(ancestor.reason, "ancestor Area");
    }

    #[test]
    fn default_parent_follows_subfolder_note_ancestry_and_rejects_cycles() {
        let vault = SyntheticVault::new(&[
            ("Area.md", AREA),
            ("zk/Idea.md", "---\nparent: [[zk/Folder]]\n---\n"),
            ("zk/Folder.md", "---\nparent: [[Area]]\n---\n"),
            ("Loop.md", "---\nparent: [[Other]]\n---\n"),
            ("Other.md", "---\nparent: [[Loop]]\n---\n"),
            ("NoParent.md", "---\nparent: [[Missing]]\n---\n"),
        ]);
        let mut store = vault.filesystem_store();
        for (route, expected) in [
            ("zk/Idea", Some("Area")),
            ("Loop", None),
            ("NoParent", None),
        ] {
            let SnapshotTarget::VaultNote(note) =
                classify_target(&mut store, route).expect("classify")
            else {
                panic!("expected note {route}");
            };
            assert_eq!(
                default_parent(&mut store, &note)
                    .map(|parent| parent.resolved.route),
                expected.map(str::to_string),
                "route {route}"
            );
        }
    }

    #[test]
    fn default_parent_walk_is_bounded_to_eight_ancestor_hops() {
        let mut files = vec![("Area.md".to_string(), AREA.to_string())];
        files.push((
            "Start.md".to_string(),
            "---\nparent: [[n1]]\n---\n".to_string(),
        ));
        for index in 1..8 {
            files.push((
                format!("n{index}.md"),
                format!("---\nparent: [[n{}]]\n---\n", index + 1),
            ));
        }
        files.push(("n8.md".to_string(), PROJECT.to_string()));
        let file_refs = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect::<Vec<_>>();
        let vault = SyntheticVault::new(&file_refs);
        let mut store = vault.filesystem_store();
        let SnapshotTarget::VaultNote(note) =
            classify_target(&mut store, "Start").expect("classify")
        else {
            panic!("expected note");
        };
        assert_eq!(
            default_parent(&mut store, &note)
                .expect("eight hops include n8")
                .resolved
                .route,
            "n8"
        );

        files.pop();
        files.push((
            "n8.md".to_string(),
            "---\nparent: [[n9]]\n---\n".to_string(),
        ));
        files.push(("n9.md".to_string(), AREA.to_string()));
        let file_refs = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect::<Vec<_>>();
        let vault = SyntheticVault::new(&file_refs);
        let mut store = vault.filesystem_store();
        let SnapshotTarget::VaultNote(note) =
            classify_target(&mut store, "Start").expect("classify")
        else {
            panic!("expected note");
        };
        assert!(default_parent(&mut store, &note).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escapes_never_become_vault_notes() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().expect("outside dir");
        let secret = outside.path().join("secret.md");
        fs::write(&secret, ORDINARY).expect("write");
        let vault = SyntheticVault::new(&[("Note.md", ORDINARY)]);
        symlink(&secret, vault.root.join("Escape.md")).expect("symlink");

        let mut store = vault.filesystem_store();
        let target = classify_target(
            &mut store,
            &vault.root.join("Escape.md").to_string_lossy(),
        )
        .expect("classify");
        assert_eq!(
            target,
            SnapshotTarget::PlainMarkdown(vault.root.join("Escape.md"))
        );
    }
}
