//! Shared Obsidian vault-note link resolution.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteIndex {
    relative_paths: BTreeSet<PathBuf>,
    basename_paths: BTreeMap<String, Option<PathBuf>>,
}

impl NoteIndex {
    pub(crate) fn from_paths<I>(paths: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let mut relative_paths = BTreeSet::new();
        let mut basename_paths = BTreeMap::new();
        for path in paths {
            if let Some(name) = markdown_basename(&path) {
                match basename_paths.entry(name.to_lowercase()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(Some(path.clone()));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().as_ref() != Some(&path) {
                            entry.insert(None);
                        }
                    }
                }
            }
            relative_paths.insert(path);
        }
        Self {
            relative_paths,
            basename_paths,
        }
    }

    pub(crate) fn resolve(
        &self,
        current_path: Option<&Path>,
        target: &str,
    ) -> Option<PathBuf> {
        self.resolve_detailed(current_path, target)
            .path()
            .map(Path::to_path_buf)
    }

    fn resolve_detailed(
        &self,
        current_path: Option<&Path>,
        target: &str,
    ) -> LinkResolution {
        if target.is_empty() {
            return current_path
                .map(Path::to_path_buf)
                .map_or(LinkResolution::Missing, LinkResolution::Found);
        }
        let Some(candidate) = target_to_markdown_path(target) else {
            return LinkResolution::Missing;
        };
        if self.relative_paths.contains(&candidate) {
            return LinkResolution::Found(candidate);
        }
        if target.contains('/') || target.contains('\\') {
            return LinkResolution::Missing;
        }
        let basename = target
            .strip_suffix(".md")
            .or_else(|| target.strip_suffix(".MD"))
            .unwrap_or(target)
            .to_lowercase();
        match self.basename_paths.get(&basename) {
            Some(Some(path)) => LinkResolution::Found(path.clone()),
            Some(None) => LinkResolution::Ambiguous,
            None => LinkResolution::Missing,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkResolution {
    Found(PathBuf),
    Ambiguous,
    Missing,
}

impl LinkResolution {
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) => Some(path),
            Self::Ambiguous | Self::Missing => None,
        }
    }
}

/// Resolves a link's exact path without walking the vault. The basename index
/// is built only when that exact path is absent and a basename fallback can
/// apply.
pub(crate) struct VaultLinkResolver {
    root: PathBuf,
    index: OnceLock<NoteIndex>,
    #[cfg(test)]
    index_builds: std::sync::atomic::AtomicUsize,
}

impl VaultLinkResolver {
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            index: OnceLock::new(),
            #[cfg(test)]
            index_builds: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(crate) fn resolve(
        &self,
        current_path: &Path,
        target: &str,
    ) -> LinkResolution {
        if target.is_empty() {
            return LinkResolution::Found(current_path.to_path_buf());
        }
        let Some(candidate) = target_to_markdown_path(target) else {
            return LinkResolution::Missing;
        };
        if self.root.join(&candidate).is_file() {
            return LinkResolution::Found(candidate);
        }
        if target.contains('/') || target.contains('\\') {
            return LinkResolution::Missing;
        }
        self.index
            .get_or_init(|| self.build_index())
            .resolve_detailed(Some(current_path), target)
    }

    #[cfg(test)]
    fn index_was_built(&self) -> bool {
        self.index_builds.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    fn build_index(&self) -> NoteIndex {
        #[cfg(test)]
        self.index_builds
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut paths = Vec::new();
        collect_markdown_paths(&self.root, &self.root, &mut paths);
        NoteIndex::from_paths(paths)
    }
}

fn collect_markdown_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            collect_markdown_paths(root, &path, paths);
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            paths.push(relative.to_path_buf());
        }
    }
}

pub(crate) fn markdown_basename(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".md")
        .or_else(|| name.strip_suffix(".MD"))
        .or(Some(name))
}

pub(crate) fn target_to_markdown_path(target: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for component in Path::new(target).components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => return None,
        }
    }
    if !path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        let file_name = path.file_name()?.to_os_string();
        path.set_file_name(format!("{}.md", file_name.to_string_lossy()));
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn resolves_exact_path_before_unique_case_insensitive_basename() {
        let root = tempfile::tempdir().expect("temp vault");
        fs::create_dir_all(root.path().join("Projects")).expect("directory");
        fs::create_dir_all(root.path().join("Archive")).expect("directory");
        fs::write(root.path().join("Projects/Alpha.md"), "").expect("note");
        fs::write(root.path().join("Archive/alpha.md"), "").expect("note");
        fs::write(root.path().join("Solo.md"), "").expect("note");
        let resolver = VaultLinkResolver::new(root.path());
        let day = Path::new("Daily/20260928.md");

        assert_eq!(
            resolver.resolve(day, "Projects/Alpha"),
            LinkResolution::Found(PathBuf::from("Projects/Alpha.md"))
        );
        assert_eq!(resolver.resolve(day, "Alpha"), LinkResolution::Ambiguous);
        assert_eq!(
            resolver.resolve(day, "solo"),
            LinkResolution::Found(PathBuf::from("Solo.md"))
        );
    }

    #[test]
    fn exact_root_note_does_not_build_the_vault_index() {
        let root = tempfile::tempdir().expect("temp vault");
        fs::write(root.path().join("route.md"), "").expect("note");
        let resolver = VaultLinkResolver::new(root.path());

        assert_eq!(
            resolver.resolve(Path::new("daily.md"), "route"),
            LinkResolution::Found(PathBuf::from("route.md"))
        );
        assert!(!resolver.index_was_built());
        assert_eq!(
            resolver.resolve(Path::new("daily.md"), "missing"),
            LinkResolution::Missing
        );
        assert!(resolver.index_was_built());
    }

    #[test]
    fn basename_walk_skips_hidden_directories() {
        let root = tempfile::tempdir().expect("temp vault");
        fs::create_dir_all(root.path().join(".hidden")).expect("hidden dir");
        fs::write(root.path().join(".hidden/Secret.md"), "")
            .expect("hidden note");
        let resolver = VaultLinkResolver::new(root.path());

        assert_eq!(
            resolver.resolve(Path::new("daily.md"), "Secret"),
            LinkResolution::Missing
        );
    }
}
