use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::*;
use crate::native::{
    collect_done::{MovedBlockTarget, MovedBlockTargets},
    task_dependencies::dependency_id,
};

#[test]
fn cuts_removes_and_reinserts_a_same_note_line_range() {
    let original = "- First ^first\n- Move me ^moved\n- Last ^last\n";
    let range = LineRange { start: 1, end: 2 };
    let fragment = cut_line_range(original, &range).expect("valid range");
    let after_cut = remove_line_ranges(original, std::slice::from_ref(&range));
    let insertion = after_cut.find("- Last").expect("last line");
    let (result, landing_lines) =
        insert_fragments_at_byte(&after_cut, insertion, &[fragment], false)
            .expect("valid byte offset");

    assert_eq!(result, original);
    assert_eq!(landing_lines, vec![1]);
    assert!(
        insert_fragments_at_byte(&after_cut, usize::MAX, &[], false).is_none()
    );
}

#[test]
fn rebases_tabs_to_spaces_but_preserves_fenced_fragment_indentation() {
    let fragment = concat!(
        "\t- Child ^child\n",
        "\t\t- Deeper\n",
        "\t\t\t```text\n",
        "\t\t\t  keep this indentation\n",
        "\t\t\t```\n",
    );
    let rebased = rebase_fragment(fragment, "", "", "\t", "  ", true);

    assert_eq!(
        rebased,
        concat!(
            "  - Child ^child\n",
            "    - Deeper\n",
            "\t\t\t```text\n",
            "\t\t\t  keep this indentation\n",
            "\t\t\t```\n",
        )
    );
}

#[test]
fn renames_colliding_anchors_and_repairs_pathless_fragment_links() {
    let fragment = concat!(
        "- Child ^child\n",
        "  - [[#^child]]\n",
        "  - Kept identity ^child-1\n",
    )
    .to_string();
    let (mut fragments, renames, id_map) = rename_fragments(
        &[fragment],
        Path::new("dest.md"),
        &BTreeSet::from(["child".to_string()]),
        IdCollisionPolicy::Rename,
    )
    .expect("collision is renamed");
    assert_eq!(renames.len(), 1);
    assert_eq!(renames[0].new_block_id, "child-2");

    let moved = repair_moved_fragment(
        &fragments[0],
        Path::new("source.md"),
        "source",
        &NoteIndex::from_paths([
            PathBuf::from("source.md"),
            PathBuf::from("dest.md"),
        ]),
        &MovedBlockTargets::new(),
        &BTreeSet::new(),
        &id_map,
    );
    fragments[0] = moved.contents;
    assert!(fragments[0].contains("[[#^child-2]]"));
}

#[test]
fn repairs_daily_note_links_and_returns_the_complete_transplant_summary() {
    let source = PathBuf::from("source.md");
    let destination = PathBuf::from("dest.md");
    let daily = PathBuf::from("daily/2026-11-03.md");
    let originals = BTreeMap::from([
        (source.clone(), "- Source task ^child\n".to_string()),
        (destination.clone(), "# Destination\n".to_string()),
        (daily.clone(), "- [[source#^child]]\n".to_string()),
    ]);
    let old_dependency_id = dependency_id(&source, "child").unwrap();
    let new_dependency_id = dependency_id(&destination, "child-2").unwrap();
    let moved_targets = MovedBlockTargets::from([(
        (source.clone(), "child".to_string()),
        MovedBlockTarget {
            archive_target: "dest".to_string(),
            block_id: "child-2".to_string(),
            old_dependency_id,
            new_dependency_id,
        },
    )]);

    let repaired = repair_vault_postimages(
        &originals,
        &BTreeMap::from([(source.clone(), "- Remaining\n".to_string())]),
        &destination,
        "# Destination\n- Source task ^child-2\n".to_string(),
        &NoteIndex::from_paths(originals.keys().cloned()),
        &moved_targets,
        BTreeMap::new(),
    );
    let plan = finish_transplant(
        repaired,
        vec!["- Source task ^child-2\n".to_string()],
        vec![RenameRecord {
            path: destination.clone(),
            old_block_id: "child".to_string(),
            new_block_id: "child-2".to_string(),
        }],
        vec![1],
    );

    assert_eq!(
        plan.postimages.get(&daily).map(String::as_str),
        Some("- [[dest#^child-2]]\n")
    );
    assert_eq!(plan.renames.len(), 1);
    assert_eq!(plan.fragments.len(), 1);
    assert_eq!(plan.landing_lines, vec![1]);
    assert_eq!(plan.repaired_files.len(), 1);
    assert_eq!(plan.repaired_files[0].path, daily);
    assert_eq!(plan.repaired_files[0].link_count, 1);
}
