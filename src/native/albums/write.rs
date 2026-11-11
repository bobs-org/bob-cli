//! Album records-index planning for record write paths.
//!
//! Both capture and `bob records` render one album's index from a single
//! invocation-local slice: the cached catalog folder with the batch's
//! staged record postimages overlaid by vault-relative path. Nothing here
//! touches the disk; callers stage the returned edit through their guarded
//! writer and guard the preimage even when the index is unchanged.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use super::{
    grouping::{group_records, GroupingRecord},
    index::{plan_index, IndexState},
    report::resolved_plural,
    AlbumDeclaration, AlbumFinding,
};
use crate::native::{
    host_catalog::FileEdit,
    records::{
        schema::RecordDiagnosticCode, staged_record, FolderIndex, IndexedRecord,
    },
};

/// One planned album write: the preimage-bound edit when the index must
/// change, plus the reported state. `Current` and `Damaged` carry no edit;
/// callers still guard the preimage so a concurrent album edit refuses
/// instead of silently publishing a stale index. `Damaged` callers must
/// refuse the whole record write using `findings` for the path/reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumWritePlan {
    pub(crate) state: IndexState,
    pub(crate) edit: Option<FileEdit>,
    pub(crate) findings: Vec<AlbumFinding>,
}

/// Shared folder/member completeness rule with the report path: the folder
/// must have no diagnostics and no member may carry a `NoteRead`
/// diagnostic (I/O or invalid UTF-8). Readable invalid YAML stays a
/// counted member with its fallback title/status.
pub(crate) fn folder_complete(folder: &FolderIndex) -> bool {
    folder.diagnostics().is_empty()
        && folder.notes().iter().all(|note| {
            !note.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == RecordDiagnosticCode::NoteRead
            })
        })
}

/// Overlay staged record postimages onto the cached folder slice, keyed by
/// vault-relative path. Only direct Markdown members of the folder replace
/// (or join) the slice; anything else is ignored. The folder is never
/// reloaded, and replacements never append a second member for one path.
pub(crate) fn overlay_folder_members(
    folder: &FolderIndex,
    overlays: &[(PathBuf, String)],
) -> Vec<IndexedRecord> {
    let dir = Path::new(folder.directory());
    let mut members: HashMap<PathBuf, IndexedRecord> = folder
        .notes()
        .iter()
        .map(|note| (note.relative_path.clone(), note.clone()))
        .collect();
    for (relative, markdown) in overlays {
        if relative.parent() != Some(dir) {
            continue;
        }
        if !relative
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let Some(stem) = relative
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
        else {
            continue;
        };
        members.insert(
            relative.clone(),
            staged_record(relative.clone(), stem.to_owned(), markdown),
        );
    }
    let mut members: Vec<IndexedRecord> = members.into_values().collect();
    members.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    members
}

/// Render one album's records index from the overlaid slice and plan its
/// owned region against the supplied preimage bytes. The schema, folder
/// slice, and preimage must belong to the same album read; callers bind
/// them before calling.
pub(crate) fn plan_album_write(
    declaration: &AlbumDeclaration,
    album_before: &str,
    folder: &FolderIndex,
    overlays: &[(PathBuf, String)],
) -> AlbumWritePlan {
    let members = overlay_folder_members(folder, overlays);
    let grouping_records: Vec<GroupingRecord> =
        members.iter().map(GroupingRecord::from_indexed).collect();
    let schema_fields = declaration
        .record_type
        .as_ref()
        .map(|record_type| record_type.fields.as_slice())
        .unwrap_or(&[]);
    let grouping = group_records(
        schema_fields,
        declaration.index_by.as_deref(),
        &declaration.index_facets,
        &grouping_records,
    );
    let planned = plan_index(
        &declaration.path,
        Some(album_before),
        &declaration.stem,
        &resolved_plural(declaration),
        schema_fields,
        &grouping,
        declaration.index_by.as_deref(),
        members.len(),
        folder_complete(folder),
    );
    AlbumWritePlan {
        state: planned.state,
        edit: planned.edit,
        findings: planned.findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::albums::parse_album_declaration;
    use crate::native::capture::{
        write_staged_files_guarded, CaptureFailureKind, StagedTextFile,
    };
    use crate::native::host_catalog::ReadDependencies;
    use crate::native::records::RecordCatalog;

    const ALBUM: &str = concat!(
        "---\n",
        "type: [[album]]\n",
        "parent: \"[[body]]\"\n",
        "summary: Places to eat.\n",
        "record:\n",
        "  noun: restaurant\n",
        "  label: Restaurant\n",
        "  fields:\n",
        "    status:\n",
        "      required: true\n",
        "      values:\n",
        "        to_try: \"To try\"\n",
        "        liked: \"Liked\"\n",
        "    category:\n",
        "      required: true\n",
        "---\n\n",
        "# Restaurants\n",
    );

    const NINJA: &str = concat!(
        "---\n",
        "parent: \"[[eat]]\"\n",
        "type: \"[[restaurant]]\"\n",
        "title: \"Ninja Sushi\"\n",
        "status: liked\n",
        "category: Sushi\n",
        "---\n\n",
        "# Ninja Sushi\n",
    );

    fn vault() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        std::fs::create_dir_all(root.join("eat")).expect("eat folder");
        std::fs::write(root.join("eat.md"), ALBUM).expect("album");
        std::fs::write(root.join("eat/ninja_sushi.md"), NINJA).expect("record");
        temp
    }

    fn declaration(album_before: &str) -> AlbumDeclaration {
        parse_album_declaration(std::path::Path::new("eat.md"), album_before)
            .expect("album parses")
    }

    fn relative(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn staged_record_matches_disk_decode() {
        let temp = vault();
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let disk = folder
            .notes()
            .iter()
            .find(|note| note.stem == "ninja_sushi")
            .expect("disk member")
            .clone();
        let staged = staged_record(
            relative("eat/ninja_sushi.md"),
            "ninja_sushi".to_owned(),
            NINJA,
        );
        // Staged postimages carry no mtime; everything the index reads
        // must match the disk decode exactly.
        assert_eq!(staged.relative_path, disk.relative_path);
        assert_eq!(staged.stem, disk.stem);
        assert_eq!(staged.title, disk.title);
        assert_eq!(staged.display, disk.display);
        assert_eq!(staged.fields, disk.fields);
        assert_eq!(staged.diagnostics, disk.diagnostics);
    }

    #[test]
    fn overlay_replaces_by_path_without_appending() {
        let temp = vault();
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let updated = NINJA.replace("status: liked", "status: to_try");
        let members = overlay_folder_members(
            folder,
            &[(relative("eat/ninja_sushi.md"), updated)],
        );
        assert_eq!(members.len(), 1, "replace, never append");
        let status = members[0]
            .fields
            .iter()
            .find(|field| field.key == "status")
            .expect("status field");
        assert_eq!(
            status.value,
            crate::native::records::IndexedFieldValue::Scalar(
                "to_try".to_owned()
            )
        );
    }

    #[test]
    fn overlay_ignores_non_direct_members() {
        let temp = vault();
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let before = overlay_folder_members(folder, &[]);
        let members = overlay_folder_members(
            folder,
            &[
                (relative("movies/dune.md"), NINJA.to_owned()),
                (relative("eat/nested/deep.md"), NINJA.to_owned()),
                (relative("eat/notes.txt"), NINJA.to_owned()),
            ],
        );
        assert_eq!(members, before, "only direct Markdown members overlay");
    }

    #[test]
    fn plan_repairs_a_missing_index_without_touching_other_bytes() {
        let temp = vault();
        let album_before =
            std::fs::read_to_string(temp.path().join("eat.md")).expect("album");
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let planned = plan_album_write(
            &declaration(&album_before),
            &album_before,
            folder,
            &[],
        );
        let edit = planned.edit.expect("missing index plans an edit");
        assert_eq!(edit.before, album_before, "edit binds the preimage");
        assert!(edit.after.contains("Ninja Sushi"), "{}", edit.after);
        assert!(
            edit.after.contains("# Restaurants"),
            "authored bytes survive: {}",
            edit.after
        );
        assert!(
            edit.after.contains("## Records"),
            "first write inserts the section: {}",
            edit.after
        );
    }

    #[test]
    fn two_file_install_refuses_on_album_drift_and_rolls_back() {
        let temp = vault();
        let root = temp.path();
        let album_before =
            std::fs::read_to_string(root.join("eat.md")).expect("album");
        let mut catalog = RecordCatalog::new(root);
        let folder = catalog.folder("eat");
        let record_after = NINJA.replace("status: liked", "status: to_try");
        let planned = plan_album_write(
            &declaration(&album_before),
            &album_before,
            folder,
            &[(relative("eat/ninja_sushi.md"), record_after.clone())],
        );
        let edit = planned.edit.expect("stale index plans an edit");
        // Records-CLI write shape: the record create plus the album edit,
        // with the album preimage guarded beside them.
        let staged = vec![
            StagedTextFile {
                target: root.join("eat/ramen_nagi.md"),
                target_existed: false,
                original_target: String::new(),
                updated_target: record_after,
            },
            StagedTextFile {
                target: root.join("eat.md"),
                target_existed: true,
                original_target: edit.before.clone(),
                updated_target: edit.after,
            },
        ];
        let mut deps = ReadDependencies::default();
        deps.insert(relative("eat.md"), Some(album_before));
        // A concurrent album edit lands after planning.
        std::fs::write(root.join("eat.md"), format!("{ALBUM}<!-- drift -->\n"))
            .expect("drift");
        let error = write_staged_files_guarded(&staged, || deps.validate(root))
            .expect_err("drift refuses");
        assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
        assert!(
            !root.join("eat/ramen_nagi.md").exists(),
            "refusal installs nothing"
        );
        assert!(
            std::fs::read_to_string(root.join("eat.md"))
                .expect("album")
                .contains("drift"),
            "foreign bytes survive"
        );
    }

    #[test]
    fn damaged_folder_and_unreadable_member_are_damaged() {
        let temp = vault();
        let root = temp.path();
        let album_before =
            std::fs::read_to_string(root.join("eat.md")).expect("album");
        let declaration = declaration(&album_before);
        // A direct `.md` occupant that is a directory damages the slice.
        std::fs::create_dir_all(root.join("eat/blocked.md"))
            .expect("directory occupant");
        let mut catalog = RecordCatalog::new(root);
        let folder = catalog.folder("eat");
        let damaged = plan_album_write(
            &declaration,
            &album_before,
            folder,
            &[(relative("eat/new.md"), NINJA.to_owned())],
        );
        assert_eq!(damaged.state, IndexState::Damaged);
        assert!(damaged.edit.is_none());
        assert!(
            damaged.findings.iter().any(|finding| finding
                .message
                .contains("could not be read completely")
                || finding.message.contains("occupied")),
            "useful path/reason: {:?}",
            damaged.findings
        );
        // Unreadable frontmatter (invalid UTF-8, deterministic even as
        // root) also damages; readable malformed YAML does not block.
        std::fs::remove_dir_all(root.join("eat/blocked.md")).expect("cleanup");
        std::fs::write(
            root.join("eat/unreadable.md"),
            b"---\ntitle: \xff\n---\n".as_slice(),
        )
        .expect("unreadable member");
        let mut catalog = RecordCatalog::new(root);
        let folder = catalog.folder("eat");
        let unreadable = plan_album_write(
            &declaration,
            &album_before,
            folder,
            &[(relative("eat/new.md"), NINJA.to_owned())],
        );
        assert_eq!(unreadable.state, IndexState::Damaged);
        assert!(unreadable.edit.is_none());
        std::fs::remove_file(root.join("eat/unreadable.md")).expect("cleanup");
        std::fs::write(
            root.join("eat/malformed.md"),
            "---\n: bad: [unclosed\n---\n\n# Malformed\n",
        )
        .expect("malformed member");
        let mut catalog = RecordCatalog::new(root);
        let folder = catalog.folder("eat");
        let malformed = plan_album_write(
            &declaration,
            &album_before,
            folder,
            &[(relative("eat/new.md"), NINJA.to_owned())],
        );
        assert_ne!(malformed.state, IndexState::Damaged);
    }

    #[test]
    fn album_guard_refuses_even_when_no_edit_was_needed() {
        let temp = vault();
        let root = temp.path();
        let album_before =
            std::fs::read_to_string(root.join("eat.md")).expect("album");
        let mut deps = ReadDependencies::default();
        deps.insert(relative("eat.md"), Some(album_before));
        std::fs::write(root.join("eat.md"), format!("{ALBUM}<!-- drift -->\n"))
            .expect("drift");
        let error = write_staged_files_guarded(&[], || deps.validate(root))
            .expect_err("drift refuses");
        assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
        assert!(
            error
                .message
                .contains("eat.md changed on disk after planning"),
            "names the album: {}",
            error.message
        );
    }
}
