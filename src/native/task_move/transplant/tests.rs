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

#[test]
fn final_postimage_digest_carry_includes_review_only_and_split_changes() {
    let week_only_review = PathBuf::from("2026/2026-W46.md");
    let week_review_only_split = PathBuf::from("2026/2026-W47.md");
    let week_both = PathBuf::from("2026/2026-W48.md");
    let week_edited = PathBuf::from("2026/2026-W49.md");
    let week_reflected = PathBuf::from("2026/2026-W50.md");
    let target = "[[source#^moved]]";
    let replacement = "[[destination#^moved]]";

    let make_note = |summary: &str, review: &str| {
        format!(
            "---\nsummary: {summary:?}\nsummary_digest: {}\nreview_digest: {}\nsummary_drafted: 2026-11-19\n---\n# Week\n\n## Review\n\n{review}\n",
            crate::native::intentions::digest(summary),
            crate::native::periods::note::review_digest(review),
        )
    };
    let only_review =
        make_note("No summary link", &format!("Reviewed {target}."));
    let review_only_split = make_note(
        &format!("Summary remains {target}."),
        &format!("Reviewed {target}."),
    );
    let both = make_note(
        &format!("Summary {target}."),
        &format!("Reviewed {target}."),
    );
    let edited_summary = format!("Edited summary {target}.");
    let edited = format!(
        "---\nsummary: {edited_summary:?}\nsummary_digest: sha256:eeeeeeeeeeeeeeee\nsummary_drafted: 2026-11-19\n---\n# Week\n"
    );
    let reflected_summary = format!("Reflected summary {target}.");
    let reflected = format!(
        "---\nsummary: {reflected_summary:?}\nsummary_digest: {}\nsummary_drafted: 2026-11-19\nreflection: Bryan wrote this.\n---\n# Week\n",
        crate::native::intentions::digest(&reflected_summary),
    );
    let originals = BTreeMap::from([
        (week_only_review.clone(), only_review.clone()),
        (week_review_only_split.clone(), review_only_split.clone()),
        (week_both.clone(), both.clone()),
        (week_edited.clone(), edited.clone()),
        (week_reflected.clone(), reflected.clone()),
    ]);
    let postimages = BTreeMap::from([
        (
            week_only_review.clone(),
            only_review.replace(target, replacement),
        ),
        (
            week_review_only_split.clone(),
            review_only_split.replace(
                &format!("Reviewed {target}."),
                &format!("Reviewed {replacement}."),
            ),
        ),
        (week_both.clone(), both.replace(target, replacement)),
        (week_edited.clone(), edited.replace(target, replacement)),
        (
            week_reflected.clone(),
            reflected.replace(target, replacement),
        ),
    ]);
    let mut repaired_files = vec![
        RepairedFile {
            path: week_only_review.clone(),
            link_count: 1,
            dependency_token_count: 0,
        },
        RepairedFile {
            path: week_review_only_split.clone(),
            link_count: 1,
            dependency_token_count: 0,
        },
        RepairedFile {
            path: week_both.clone(),
            link_count: 2,
            dependency_token_count: 0,
        },
        RepairedFile {
            path: week_edited.clone(),
            link_count: 1,
            dependency_token_count: 0,
        },
        RepairedFile {
            path: week_reflected.clone(),
            link_count: 1,
            dependency_token_count: 0,
        },
    ];
    let mut final_postimages = postimages;
    repair_owned_postimages(
        &originals,
        &mut final_postimages,
        &mut repaired_files,
        &NoteIndex::from_paths(originals.keys().cloned()),
        &MovedBlockTargets::new(),
        std::slice::from_ref(&week_both),
    )
    .expect("all final drafts carry from captured original bytes");

    let only_review_fields = crate::native::intentions::read(
        &final_postimages[&week_only_review],
        crate::native::intentions::NoteSubject::Period(
            crate::native::periods::calendar::PeriodId::Week {
                year: 2026,
                week: 46,
            },
        ),
    );
    assert_eq!(
        only_review_fields.review_digest.value.as_deref(),
        Some(
            crate::native::periods::note::review_digest(
                "Reviewed [[destination#^moved]]."
            )
            .as_str()
        )
    );
    assert_eq!(
        only_review_fields.summary_digest.value.as_deref(),
        Some(crate::native::intentions::digest("No summary link").as_str())
    );

    let review_only_fields = crate::native::intentions::read(
        &final_postimages[&week_review_only_split],
        crate::native::intentions::NoteSubject::Period(
            crate::native::periods::calendar::PeriodId::Week {
                year: 2026,
                week: 47,
            },
        ),
    );
    assert_eq!(
        review_only_fields.review_digest.value.as_deref(),
        Some(
            crate::native::periods::note::review_digest(
                "Reviewed [[destination#^moved]]."
            )
            .as_str()
        )
    );
    assert_eq!(
        review_only_fields.summary_digest.value.as_deref(),
        Some(
            crate::native::intentions::digest(
                "Summary remains [[source#^moved]]."
            )
            .as_str()
        )
    );

    let both_fields = crate::native::intentions::read(
        &final_postimages[&week_both],
        crate::native::intentions::NoteSubject::Period(
            crate::native::periods::calendar::PeriodId::Week {
                year: 2026,
                week: 48,
            },
        ),
    );
    assert_eq!(
        both_fields.review_digest.value.as_deref(),
        Some(
            crate::native::periods::note::review_digest(
                "Reviewed [[destination#^moved]]."
            )
            .as_str()
        )
    );
    assert_eq!(
        both_fields.summary_digest.value.as_deref(),
        Some(
            crate::native::intentions::digest(
                "Summary [[destination#^moved]]."
            )
            .as_str()
        )
    );
    assert_eq!(
        crate::native::intentions::state::summary_state(&both_fields),
        crate::native::intentions::state::SummaryState::Drafted,
        "the final postimage keeps the complete weekly draft"
    );
    let edited_fields = crate::native::intentions::read(
        &final_postimages[&week_edited],
        crate::native::intentions::NoteSubject::Period(
            crate::native::periods::calendar::PeriodId::Week {
                year: 2026,
                week: 49,
            },
        ),
    );
    assert_eq!(
        edited_fields.summary_digest.value.as_deref(),
        Some("sha256:eeeeeeeeeeeeeeee"),
        "an edited summary never acquires a refreshed ownership digest"
    );
    let reflected_fields = crate::native::intentions::read(
        &final_postimages[&week_reflected],
        crate::native::intentions::NoteSubject::Period(
            crate::native::periods::calendar::PeriodId::Week {
                year: 2026,
                week: 50,
            },
        ),
    );
    assert_eq!(
        reflected_fields.summary_digest.value.as_deref(),
        Some(crate::native::intentions::digest(&reflected_summary).as_str()),
        "a reflected summary never acquires a refreshed ownership digest"
    );
    assert_eq!(
        repaired_files
            .iter()
            .find(|file| file.path == week_only_review)
            .unwrap()
            .link_count,
        1,
        "a digest stamp is not counted as another rewritten link"
    );
    assert_eq!(
        repaired_files
            .iter()
            .find(|file| file.path == week_both)
            .unwrap()
            .link_count,
        2,
        "post-insertion digest carry does not add another link count"
    );

    let unchanged_path = PathBuf::from("2026/2026-W51.md");
    let unchanged = make_note(
        "An unchanged summary",
        "**Headline:** No movement\n\nNo changed links.",
    );
    let unchanged_originals =
        BTreeMap::from([(unchanged_path.clone(), unchanged.clone())]);
    let mut unchanged_postimages = BTreeMap::new();
    let mut unchanged_repairs = Vec::new();
    repair_owned_postimages(
        &unchanged_originals,
        &mut unchanged_postimages,
        &mut unchanged_repairs,
        &NoteIndex::from_paths([unchanged_path]),
        &MovedBlockTargets::new(),
        &[],
    )
    .expect("no-op postimage remains valid");
    assert!(unchanged_postimages.is_empty());
    assert!(unchanged_repairs.is_empty());
}
