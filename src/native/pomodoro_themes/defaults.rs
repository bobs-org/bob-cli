//! Disposable note facts for the explicit list, never completion/parse paths.
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{hints::UnknownDefaultHint, Catalog};
use crate::native::{
    collect_done,
    plan_budget::normalize_component,
    pomodoro_homes::{Hue, NoteFacts, NoteKind, NoteTheme},
    projects,
};

/// v2: `area_hue` distinguishes a grey area (`Some(None)`) from a
/// non-area note (`None`); v1 caches hashed `grey` as an invalid value.
const VERSION: u32 = 2;
type Fingerprint = (u64, u64);

#[derive(Default, Serialize, Deserialize)]
struct Directory {
    fingerprint: Option<Fingerprint>,
    directories: BTreeMap<String, Directory>,
    files: BTreeMap<String, File>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    fingerprint: Option<Fingerprint>,
    default: Option<String>,
    /// `None` (field omitted) for a non-area note; `Some(None)` (`null`)
    /// for a grey top-level area.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_area_hue"
    )]
    area_hue: Option<Option<Hue>>,
}

/// A present `area_hue` key is a top-level area, even when its value is
/// `null` (grey); only an omitted key means "not an area".
fn present_area_hue<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Hue>>, D::Error> {
    Option::<Hue>::deserialize(deserializer).map(Some)
}

#[derive(Serialize, Deserialize)]
struct Cache {
    version: u32,
    root: Directory,
}

pub(super) struct NoteDefaults {
    defaults: Vec<(String, String)>,
    pub(super) areas: BTreeMap<String, Option<Hue>>,
}

impl NoteDefaults {
    pub(super) fn unknown(&self, catalog: &Catalog) -> Vec<UnknownDefaultHint> {
        self.defaults
            .iter()
            .filter_map(|(path, name)| {
                let key = normalize_component(name);
                if catalog.theme_ref(&key, name).known {
                    return None;
                }
                Some(UnknownDefaultHint {
                    code: "theme_default_unknown",
                    path: path.clone(),
                    similar_to: catalog.similar_to(name),
                    name: name.clone(),
                })
            })
            .collect()
    }
}

pub(super) fn load(
    root: &Path,
    fresh: bool,
    cache_dir: &Path,
) -> io::Result<NoteDefaults> {
    // The vault itself may be a symlink; fingerprint its target directory.
    let root = root.canonicalize()?;
    let path = cache_path(&root, cache_dir);
    let previous = if fresh {
        None
    } else {
        fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
            .filter(|cache| {
                cache.version == VERSION
                    && valid_names(&cache.root, Path::new(""))
            })
    };
    let mut dirty = previous.is_none();
    let mut cache = previous.unwrap_or(Cache {
        version: VERSION,
        root: Directory::default(),
    });
    let mut files = Vec::new();
    discover(
        &root,
        Path::new(""),
        &mut cache.root,
        &mut files,
        &mut dirty,
    )?;
    let (mut defaults, files_changed) = refresh_files(&root, &mut files)?;
    dirty |= files_changed;
    defaults.sort();
    let areas = cache
        .root
        .files
        .iter()
        .filter_map(|(name, file)| {
            file.area_hue.map(|hue| {
                (normalize_component(name.trim_end_matches(".md")), hue)
            })
        })
        .collect();
    if dirty && let Ok(text) = serde_json::to_string(&cache) {
        // Cache failures never make a read-only report unavailable.
        let _ = collect_done::atomic_write(&path, &text);
    }
    Ok(NoteDefaults { defaults, areas })
}

fn cache_path(root: &Path, cache_dir: &Path) -> PathBuf {
    let hash =
        hex::encode(Sha256::digest(super::cache::vault_key(root).as_bytes()));
    cache_dir
        .join("themes")
        .join(format!("note-defaults-v1-{hash}.json"))
}

fn valid_names(dir: &Directory, relative: &Path) -> bool {
    let valid = |name: &str| {
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\'])
    };
    dir.files
        .keys()
        .all(|name| valid(name) && projects::is_markdown_file(Path::new(name)))
        && dir.directories.iter().all(|(name, child)| {
            let child_rel = relative.join(name);
            valid(name)
                && !projects::is_excluded_directory(Path::new(name))
                && !crate::native::done_archive::is_archive_root(&child_rel)
                && valid_names(child, &child_rel)
        })
}

fn fingerprint(metadata: &fs::Metadata) -> io::Result<Fingerprint> {
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Ok((
        u64::try_from(modified.as_nanos()).unwrap_or(u64::MAX),
        metadata.len(),
    ))
}

fn discover<'a>(
    root: &Path,
    relative: &Path,
    cache: &'a mut Directory,
    files: &mut Vec<(PathBuf, &'a mut File)>,
    dirty: &mut bool,
) -> io::Result<()> {
    let directory = root.join(relative);
    let current = fingerprint(&fs::symlink_metadata(&directory)?)?;
    if cache.fingerprint != Some(current) {
        // Directory fingerprints govern discovery only. Every known file is
        // still statted by refresh_files: an edit need not touch its directory.
        let mut old_dirs = std::mem::take(&mut cache.directories);
        let mut old_files = std::mem::take(&mut cache.files);
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let Some(name) = entry.file_name().to_str().map(str::to_string)
            else {
                continue;
            };
            if kind.is_dir()
                && !projects::is_excluded_directory(&path)
                && !crate::native::done_archive::skip_archive_directory(
                    root, &path,
                )
            {
                cache.directories.insert(
                    name.clone(),
                    old_dirs.remove(&name).unwrap_or_default(),
                );
            } else if kind.is_file() && projects::is_markdown_file(&path) {
                cache.files.insert(
                    name.clone(),
                    old_files.remove(&name).unwrap_or_default(),
                );
            }
        }
        cache.fingerprint = Some(current);
        *dirty = true;
    }
    for (name, file) in &mut cache.files {
        files.push((relative.join(name), file));
    }
    for (name, child) in &mut cache.directories {
        discover(root, &relative.join(name), child, files, dirty)?;
    }
    Ok(())
}

type RefreshResult = (Vec<(String, String)>, bool);

fn refresh_files(
    root: &Path,
    files: &mut [(PathBuf, &mut File)],
) -> io::Result<RefreshResult> {
    // Bound both thread count and the minimum work per thread. In particular,
    // small vaults stay sequential. Each file belongs to exactly one worker;
    // every call still checks every file's mtime/size, including untyped notes.
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(4)
        .min(files.len() / 256)
        .max(1);
    if workers == 1 {
        return refresh_chunk(root, files);
    }
    let chunk_size = files.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks_mut(chunk_size)
            .map(|chunk| scope.spawn(move || refresh_chunk(root, chunk)))
            .collect();
        let mut defaults = Vec::new();
        let mut dirty = false;
        for handle in handles {
            let (rows, changed) =
                handle.join().expect("note facts worker panicked")?;
            defaults.extend(rows);
            dirty |= changed;
        }
        Ok((defaults, dirty))
    })
}

fn refresh_chunk(
    root: &Path,
    files: &mut [(PathBuf, &mut File)],
) -> io::Result<RefreshResult> {
    let mut defaults = Vec::new();
    let mut dirty = false;
    for (relative, file) in files {
        let path = root.join(&*relative);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() {
            continue;
        }
        let current = fingerprint(&metadata)?;
        if file.fingerprint != Some(current) {
            let contents = fs::read_to_string(&path)?;
            let facts = NoteFacts::parse(&contents);
            file.default = match (facts.kind, facts.theme) {
                (
                    NoteKind::Area | NoteKind::Project,
                    Some(NoteTheme::Name(name)),
                ) => Some(name),
                _ => None,
            };
            file.area_hue = if relative.parent() == Some(Path::new("")) {
                super::scan::root_area_hue(
                    relative
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .trim_end_matches(".md"),
                    &contents,
                )
            } else {
                None
            };
            file.fingerprint = Some(current);
            dirty = true;
        }
        if let Some(name) = &file.default {
            defaults.push((
                relative.to_string_lossy().replace('\\', "/"),
                name.clone(),
            ));
        }
    }
    Ok((defaults, dirty))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn cached_area_hues_keep_grey_apart_from_non_areas() {
        let vault = tempfile::tempdir().expect("vault");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(
            vault.path().join("quiet.md"),
            "---\ntype: [[area]]\nparent: [[area]]\ncolor: grey\n---\n",
        )
        .unwrap();
        fs::write(
            vault.path().join("dev.md"),
            "---\ntype: [[area]]\nparent: [[area]]\ncolor: blue\n---\n",
        )
        .unwrap();
        fs::write(
            vault.path().join("sase.md"),
            "---\ntype: [[project]]\nparent: [[dev]]\n---\n",
        )
        .unwrap();
        let expected = BTreeMap::from([
            ("dev".to_string(), Some(Hue::Blue)),
            ("quiet".to_string(), None),
        ]);

        let fresh = load(vault.path(), true, cache.path()).expect("fresh");
        assert_eq!(fresh.areas, expected);
        let cached = load(vault.path(), false, cache.path()).expect("cached");
        assert_eq!(cached.areas, expected);

        let root = vault.path().canonicalize().unwrap();
        let text = fs::read_to_string(cache_path(&root, cache.path())).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], VERSION);
        let files = &value["root"]["files"];
        assert!(files["quiet.md"]["area_hue"].is_null());
        assert!(files["quiet.md"].get("area_hue").is_some());
        assert_eq!(files["dev.md"]["area_hue"], "blue");
        assert!(files["sase.md"].get("area_hue").is_none());
    }
}
