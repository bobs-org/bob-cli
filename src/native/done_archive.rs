//! One done/ path contract for bob-cli.
//!
//! A vault path is a `done/` archive when its first normal path component
//! equals `done` exactly and case-sensitively. Walkers skip only the
//! top-level `done/` directory.

use std::{
    ffi::OsStr,
    path::{Component, Path},
};

/// Whether a vault-relative path lives under the top-level `done/` archive.
///
/// The first normal path component must equal `done` exactly and
/// case-sensitively. A root `done.md`, a nested `x/done/`, `Done/`, and
/// `.done/` are not archives.
pub(crate) fn is_archive_path(path: &Path) -> bool {
    first_normal_component(path) == Some(OsStr::new("done"))
}

/// Whether `relative` is exactly the vault-root `done` directory.
pub(crate) fn is_archive_root(relative: &Path) -> bool {
    let mut normals =
        relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name),
                _ => None,
            });
    matches!(normals.next(), Some(name) if name == OsStr::new("done"))
        && normals.next().is_none()
}

/// Skip this directory when walking notes: only the top-level `done/`
/// tree, never a nested folder named `done`.
pub(crate) fn skip_archive_directory(vault: &Path, directory: &Path) -> bool {
    directory.strip_prefix(vault).is_ok_and(is_archive_root)
}

fn first_normal_component(path: &Path) -> Option<&OsStr> {
    path.components().find_map(|component| match component {
        Component::Normal(name) => Some(name),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;

    #[test]
    fn shared_vectors_pin_first_component_rule() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/done_archive_paths/vectors.json"
        ))
        .expect("shared done-archive path vectors are valid JSON");
        for vector in vectors.as_array().expect("vector list") {
            let path = vector["path"].as_str().expect("path");
            let expected = vector["archive"].as_bool().expect("archive");
            assert_eq!(
                is_archive_path(Path::new(path)),
                expected,
                "{}",
                vector["id"].as_str().unwrap_or(path)
            );
        }
    }

    #[test]
    fn nested_projects_done_is_not_an_archive_or_walker_skip() {
        let vault = Path::new("/vault");
        assert!(skip_archive_directory(vault, &vault.join("done")));
        assert!(!skip_archive_directory(
            vault,
            &vault.join("projects").join("done")
        ));
        assert!(!is_archive_path(Path::new("projects/done/x.md")));
        assert!(!is_archive_root(Path::new("projects/done")));
        assert!(is_archive_root(Path::new("done")));
    }

    #[test]
    fn markdown_files_skip_top_level_done_and_keep_nested_done() {
        let temp = tempfile::tempdir().expect("vault");
        let vault = temp.path();
        fs::create_dir_all(vault.join("done")).expect("top-level done");
        fs::create_dir_all(vault.join("projects").join("done"))
            .expect("nested done");
        fs::write(vault.join("area.md"), "# area\n").expect("area");
        fs::write(vault.join("done").join("archive.md"), "# archive\n")
            .expect("archive");
        fs::write(
            vault.join("projects").join("done").join("nested.md"),
            "# nested\n",
        )
        .expect("nested");

        let files = crate::native::task_status_hooks::markdown_files(vault)
            .expect("walk");
        let relatives: Vec<_> = files
            .iter()
            .filter_map(|path| path.strip_prefix(vault).ok())
            .map(Path::to_path_buf)
            .collect();
        assert!(relatives.iter().any(|path| path == Path::new("area.md")));
        assert!(relatives
            .iter()
            .any(|path| { path == Path::new("projects/done/nested.md") }));
        assert!(!relatives.iter().any(|path| is_archive_path(path)));
    }
}
