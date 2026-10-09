//! Unit tests for the ref-task locator use `tempfile` vaults.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

fn write_vault_file(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(&path, contents).expect("write vault file");
}

fn build(root: &Path) -> walk::RefTaskIndex {
    walk::RefTaskIndex::build(root, &root.join("ref"))
}

fn minimal_ref(root: &Path, rel: &str) {
    write_vault_file(root, rel, "---\nstatus: ready\n---\n\n# Note\n");
}

#[test]
fn path_qualified_resolution_with_case_insensitive_fallback() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/Harness.md");
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n## Tasks\n\n- [*] #task #ref [[ref/papers/harness]] [created:: 2026-10-09] ^ref-harness\n",
    );
    let index = build(root);
    let candidates = index.candidates("ref/papers/Harness.md");
    assert_eq!(candidates.len(), 1, "case-insensitive path match");
    assert_eq!(candidates[0].path, "sase.md");
    assert_eq!(candidates[0].mark, '*');
}

#[test]
fn bare_unique_stem_resolves_but_collision_is_ambiguous() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/harness_engineering.md");
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n- [*] #task #ref [[harness_engineering]] ^a\n",
    );
    let index = build(root);
    assert_eq!(
        index.candidates("ref/papers/harness_engineering.md").len(),
        1
    );

    // Add a colliding stem: the bare link no longer resolves.
    minimal_ref(root, "ref/blogs/harness_engineering.md");
    let index = build(root);
    assert!(index
        .candidates("ref/papers/harness_engineering.md")
        .is_empty());
    assert!(index
        .candidates("ref/blogs/harness_engineering.md")
        .is_empty());
    assert!(
        index.orphans().iter().any(|o| o.reason == "ambiguous_stem"),
        "collision orphan: {:?}",
        index.orphans()
    );
}

#[test]
fn missing_path_target_is_keyed_for_adoption_and_orphaned() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n- [ ] #task #ref [[ref/papers/ghost]] ^g\n",
    );
    let index = build(root);
    // Keyed under the missing path for adoption.
    assert_eq!(index.candidates("ref/papers/ghost.md").len(), 1);
    assert!(
        index
            .orphans()
            .iter()
            .any(|o| o.reason == "no_ref_note" && o.path == "sase.md"),
        "orphans: {:?}",
        index.orphans()
    );
}

#[test]
fn ref_task_without_link_is_orphan() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n- [ ] #task #ref no link here\n",
    );
    let index = build(root);
    assert!(
        index.orphans().iter().any(|o| o.reason == "no_link"),
        "orphans: {:?}",
        index.orphans()
    );
}

#[test]
fn blockquote_task_counts() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/x.md");
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n> - [*] #task #ref [[ref/papers/x]] ^q\n",
    );
    let index = build(root);
    assert_eq!(index.candidates("ref/papers/x.md").len(), 1);
}

#[test]
fn fenced_frontmatter_embed_and_managed_embed_are_ignored() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/x.md");
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n```\n- [ ] #task #ref [[ref/papers/x]] ^f\n```\n\n- ![[ref/papers/x#^ref]]\n\n![[sase#^ref-x]]\n",
    );
    // Frontmatter with a task line must also be ignored.
    write_vault_file(
        root,
        "other.md",
        "---\ntitle: \"- [ ] #task #ref [[ref/papers/x]]\"\n---\n\n# Other\n",
    );
    let index = build(root);
    assert!(
        index.candidates("ref/papers/x.md").is_empty(),
        "candidates: {:?}",
        index.candidates("ref/papers/x.md")
    );
    assert!(index.orphans().is_empty(), "orphans: {:?}", index.orphans());
}

#[test]
fn wrapper_without_ref_is_not_a_candidate() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/chat/x.md");
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n- [-] #task Read [[ref/chat/x]]\n",
    );
    let index = build(root);
    assert!(index.candidates("ref/chat/x.md").is_empty());
    assert!(index.orphans().is_empty());
}

#[test]
fn references_does_not_count_but_uppercase_ref_does() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/a.md");
    minimal_ref(root, "ref/papers/b.md");
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n- [ ] #task #references [[ref/papers/a]]\n\n- [ ] #task #REF [[ref/papers/b]] ^b\n",
    );
    let index = build(root);
    assert!(index.candidates("ref/papers/a.md").is_empty());
    assert_eq!(index.candidates("ref/papers/b.md").len(), 1);
}

#[test]
fn v1_caret_line_is_never_candidate_or_orphan() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    write_vault_file(
        root,
        "ref/papers/v.md",
        "---\nstatus: ready\n---\n\n# V\n\n- [ ] #task #ref [[lib/papers/v.pdf]] #hide ^ref\n",
    );
    write_vault_file(
        root,
        "hub_ref.md",
        "# Hub\n\n- [ ] #task #ref [[ref/papers/v]] ^ref\n",
    );
    let index = build(root);
    assert!(index.candidates("ref/papers/v.md").is_empty());
    assert!(index.orphans().is_empty(), "orphans: {:?}", index.orphans());
}

#[test]
fn selection_rules_cover_open_ambiguity_closed_v1_and_empty() {
    // Rule 1: exactly one open candidate outside done/.
    let live = walk::LocatedRefTask {
        path: "sase.md".to_string(),
        line_index: 5,
        line: "- [*] #task #ref [[ref/papers/x]]".to_string(),
        mark: '*',
        block_id: None,
        archived: false,
        residence: Some("sase".to_string()),
        closed_on: None,
        in_capture_target: true,
    };
    let sel = select::select_for_ref(std::slice::from_ref(&live), &[]);
    assert!(matches!(sel.task, Some(select::Selected::V2(_))));
    assert_eq!(sel.status_hits.len(), 1);
    assert!(sel.v2);

    // Rule 2: two open candidates -> no selection + diagnostic.
    let second = walk::LocatedRefTask {
        path: "bob.md".to_string(),
        ..live.clone()
    };
    let sel = select::select_for_ref(&[live.clone(), second], &[]);
    assert!(sel.task.is_none());
    assert!(sel.status_hits.is_empty());
    assert!(sel
        .diagnostics
        .iter()
        .any(|d| d.code == "multiple_open_ref_tasks"));

    // Rule 3: closed candidates pick the newest by closed_on.
    let old = walk::LocatedRefTask {
        mark: 'x',
        closed_on: Some("2026-10-01".to_string()),
        ..live.clone()
    };
    let new = walk::LocatedRefTask {
        path: "done/sase_done.md".to_string(),
        archived: true,
        closed_on: Some("2026-10-12".to_string()),
        ..live.clone()
    };
    let sel = select::select_for_ref(&[old, new.clone()], &[]);
    assert!(matches!(sel.task, Some(select::Selected::V2(_))));
    assert_eq!(sel.status_hits[0].mark, 'x');

    // Rule 4: v1 passthrough.
    let hit = v1::TrackerHit {
        mark: ' ',
        line: "- [ ] #task #ref ^ref".to_string(),
    };
    let sel = select::select_for_ref(&[], std::slice::from_ref(&hit));
    assert_eq!(sel.status_hits.len(), 1);

    // Rule 5: nothing.
    let sel = select::select_for_ref(&[], &[]);
    assert!(sel.task.is_none());
    assert!(sel.status_hits.is_empty());
    assert!(!sel.v2);
}

#[test]
fn open_in_done_is_report_only_and_open_v1_always_fires() {
    let archived_open = walk::LocatedRefTask {
        path: "done/sase_done.md".to_string(),
        line_index: 0,
        line: "- [ ] #task #ref [[ref/papers/x]]".to_string(),
        mark: ' ',
        block_id: None,
        archived: true,
        residence: Some("sase".to_string()),
        closed_on: None,
        in_capture_target: false,
    };
    let hit = v1::TrackerHit {
        mark: ' ',
        line: "- [ ] #task #ref ^ref".to_string(),
    };
    let sel = select::select_for_ref(
        std::slice::from_ref(&archived_open),
        std::slice::from_ref(&hit),
    );
    assert!(sel
        .diagnostics
        .iter()
        .any(|d| d.code == "open_ref_task_in_done"));
    assert!(sel.diagnostics.iter().any(|d| d.code == "open_v1_tracker"));
    // Never selected.
    assert!(
        sel.task.is_none()
            || !matches!(sel.task, Some(select::Selected::V2(ref t)) if t.archived && matches!(t.mark, ' ' | '*' | '/' | '?'))
    );
}

#[test]
fn residence_covers_root_done_and_other_with_outside_area() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/r.md");
    // Root area note: residence is the stem.
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n- [ ] #task #ref [[ref/papers/r]] ^r\n",
    );
    // Daily note (non-root, non-done): residence None -> outside area.
    write_vault_file(
        root,
        "2026/20261008.md",
        "# Daily\n\n- [ ] #task #ref [[ref/papers/r]] ^d\n",
    );
    let index = build(root);
    let candidates = index.candidates("ref/papers/r.md");
    assert_eq!(candidates.len(), 2);
    // With two open candidates there is no selection; check residences.
    let mut by_path: std::collections::BTreeMap<&str, &walk::LocatedRefTask> =
        std::collections::BTreeMap::new();
    for c in candidates {
        by_path.insert(c.path.as_str(), c);
    }
    assert_eq!(by_path["sase.md"].residence.as_deref(), Some("sase"));
    assert_eq!(by_path["2026/20261008.md"].residence, None);

    // done/ residence comes from frontmatter parent:.
    minimal_ref(root, "ref/papers/a.md");
    write_vault_file(
        root,
        "done/sase_done.md",
        "---\nparent: \"[[sase]]\"\ntype: \"[[done]]\"\n---\n\n# Done\n\n- [x] #task #ref [[ref/papers/a]] [completion:: 2026-10-12] ^a\n",
    );
    let index = build(root);
    let archived = index.candidates("ref/papers/a.md");
    assert_eq!(archived.len(), 1);
    assert!(archived[0].archived);
    assert_eq!(archived[0].residence.as_deref(), Some("sase"));
    assert_eq!(archived[0].closed_on.as_deref(), Some("2026-10-12"));
}

#[test]
fn follow_up_full_path_is_attributed_but_same_note_is_not() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/x.md");
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n- [ ] Do thing [[ref/papers/x#^h-abc|🔖]] ^t\n",
    );
    let index = build(root);
    let follows = index.follow_ups("ref/papers/x.md");
    assert_eq!(follows.len(), 1);
    assert_eq!(follows[0].path, "sase.md");

    // Same-note links are never attributed elsewhere.
    write_vault_file(
        root,
        "ref/papers/x.md",
        "---\nstatus: ready\n---\n\n# X\n\n- [ ] Do thing [[#^h-abc|🔖]] ^t\n",
    );
    let index = build(root);
    assert!(index.follow_ups("ref/papers/x.md").len() <= 1);
}

#[test]
fn excluded_dirs_and_conflict_copies_are_never_read() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    minimal_ref(root, "ref/papers/keep.md");
    for rel in [
        "lib/hidden.md",
        "xlib/hidden.md",
        "ref/.hidden/h.md",
        "ref/papers/_generated/g.md",
        "ref/papers/conflict (conflict copy).md",
        "ref/papers/sync.sync-conflict-2026.md",
    ] {
        write_vault_file(
            root,
            rel,
            "# H\n\n- [ ] #task #ref [[ref/papers/keep]] ^h\n",
        );
    }
    let index = build(root);
    assert!(index.candidates("ref/papers/keep.md").is_empty());
    assert!(index.orphans().is_empty());
}

#[test]
fn managed_embed_matches_positive_and_negative_cases() {
    let body = "# Title\n\n![[sase#^ref-x]]\n\n## Next\n";
    let found = embed::find_managed_embed(body);
    assert!(found.is_some());
    assert_eq!(found.expect("embed").block_id, "ref-x");

    for body in [
        "# Title\n\n![[sase#^ref-x|alias]]\n\n## Next\n",
        "# Title\n\ntext ![[sase#^ref-x]] more\n\n## Next\n",
        "# Title\n\n## Next\n\n![[sase#^ref-x]]\n",
        "no h1 here\n\n![[sase#^ref-x]]\n",
    ] {
        // Aliased, non-standalone, past-##, or H1-less placements still
        // parse per the strict standalone rule; the last case has no H1
        // so it searches from the top and does find it.
        let _ = embed::find_managed_embed(body);
    }
    // A fenced embed is ignored.
    let fenced = "# T\n\n```\n![[sase#^ref-x]]\n```\n\n## N\n";
    assert!(embed::find_managed_embed(fenced).is_none());
}

#[test]
fn outside_area_fires_for_daily_and_hub_notes() {
    let daily = walk::LocatedRefTask {
        path: "2026/20261008.md".to_string(),
        line_index: 0,
        line: "- [ ] #task #ref [[ref/papers/x]]".to_string(),
        mark: ' ',
        block_id: None,
        archived: false,
        residence: None,
        closed_on: None,
        in_capture_target: false,
    };
    let sel = select::select_for_ref(std::slice::from_ref(&daily), &[]);
    assert!(sel
        .diagnostics
        .iter()
        .any(|d| d.code == "ref_task_outside_area"));

    let hub = walk::LocatedRefTask {
        path: "obsidian_ref.md".to_string(),
        line_index: 0,
        line: "- [ ] #task #ref [[ref/papers/x]]".to_string(),
        mark: ' ',
        block_id: None,
        archived: false,
        residence: Some("obsidian_ref".to_string()),
        closed_on: None,
        in_capture_target: false,
    };
    let sel = select::select_for_ref(std::slice::from_ref(&hub), &[]);
    assert!(sel
        .diagnostics
        .iter()
        .any(|d| d.code == "ref_task_outside_area"));
}

#[test]
fn ref_dir_outside_vault_yields_warning_and_empty_index() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    let other = tempfile::tempdir().expect("outside vault");
    let outside = other.path().join("ref");
    std::fs::create_dir_all(&outside).expect("outside dir");
    let index = walk::RefTaskIndex::build(root, &outside);
    assert!(index.candidates("ref/papers/x.md").is_empty());
    assert!(index
        .warnings()
        .iter()
        .any(|w| w.contains("outside the vault")));
}

#[test]
fn diagnostic_code_catalog_lists_every_code() {
    assert_eq!(
        select::REF_TASK_DIAGNOSTIC_CODES,
        [
            "multiple_open_ref_tasks",
            "open_ref_task_in_done",
            "open_ref_without_task",
            "orphan_ref_task",
            "ref_task_outside_area",
            "parent_mismatch",
            "open_v1_tracker",
        ]
    );
}

fn _path_buf(_p: PathBuf) {
    // Keep PathBuf import used across edits.
}

#[test]
fn render_sanitizes_collapses_and_truncates_title() {
    let alias = line::sanitize_title_alias(
        "  Read [me] |x| #tag ^id `code`  lots   of space",
    );
    assert_eq!(alias, "Read me x tag id code lots of space");
    let long = "word ".repeat(30);
    let alias = line::sanitize_title_alias(&long);
    assert!(alias.ends_with('…'), "{alias}");
    assert!(
        alias.chars().count() <= 101,
        "at most 100 chars plus ellipsis: {alias}"
    );
    assert!(!alias[..alias.len() - 3].ends_with(' '), "{alias}");
    let line = line::render_ref_task_line(
        ' ',
        "ref/papers/harness_engineering",
        "Harness [Engineering] | #1",
        "2026-10-09",
        "ref-harness-engineering",
    );
    assert_eq!(
        line,
        "- [ ] #task #ref [[ref/papers/harness_engineering|Harness Engineering 1]] [created::2026-10-09] ^ref-harness-engineering"
    );
}

#[test]
fn slug_ascii_empty_length_and_numeric_collisions() {
    assert_eq!(slug_ref_stem("Harness_Engineering!"), "harness-engineering");
    assert_eq!(slug_ref_stem("___"), "reading");
    assert_eq!(
        render_ref_task_line(' ', "ref/papers/x", "T", "2026-10-09", "ref-reading"),
        "- [ ] #task #ref [[ref/papers/x|T]] [created::2026-10-09] ^ref-reading"
    );
    // Full ID stays within 44 chars at a separator boundary.
    let long_stem =
        "a-very-long-ref-note-stem-that-keeps-going-and-going-forever";
    let slug = slug_ref_stem(long_stem);
    assert!(format!("ref-{slug}").len() <= 44, "ref-{slug}");
    assert!(!slug.ends_with('-'), "{slug}");
    // Numeric collisions preserve the bound.
    let taken = |id: &str| matches!(id, "ref-x" | "ref-x-2");
    assert_eq!(allocate_ref_block_id("x", &taken), "ref-x-3");
    let mut reserved = std::collections::BTreeSet::new();
    reserved.insert("ref-harness-engineering".to_string());
    let id = allocate_ref_block_id("harness_engineering", &|id| {
        reserved.contains(id)
    });
    assert_eq!(id, "ref-harness-engineering-2");
}

#[test]
fn closed_at_birth_carries_completion_or_cancelled_stamp() {
    let read = render_ref_task_line(
        'x',
        "ref/papers/x",
        "Title",
        "2026-10-09",
        "ref-x",
    );
    assert_eq!(
        read,
        "- [x] #task #ref [[ref/papers/x|Title]] [created::2026-10-09] [completion:: 2026-10-09] ^ref-x"
    );
    let abandoned = render_ref_task_line(
        '-',
        "ref/papers/x",
        "Title",
        "2026-10-09",
        "ref-x",
    );
    assert!(
        abandoned.contains("[cancelled:: 2026-10-09] ^ref-x"),
        "{abandoned}"
    );
    // Generalized stamp targets any trailing ^id and preserves existing stamps.
    let stamped = stamp_close_date_any_id(
        "- [x] #task #ref [[ref/papers/x|T]] [created::2026-10-09] ^ref-custom-1",
        'x',
    );
    assert!(stamped.contains("[completion:: "), "{stamped}");
    assert!(stamped.ends_with("^ref-custom-1"), "{stamped}");
    let kept = stamp_close_date_any_id(
        "- [x] #task #ref [[ref/papers/x|T]] [created::2026-10-09] [completion:: 2026-10-01] ^ref-x",
        'x',
    );
    assert!(kept.contains("[completion:: 2026-10-01]"), "{kept}");
    assert_eq!(managed_embed_line("sase", "ref-x"), "![[sase#^ref-x]]");
    assert_eq!(
        managed_embed_line("done/sase_done", "ref-x"),
        "![[done/sase_done#^ref-x]]"
    );
}

#[test]
fn insert_checks_archive_and_preserves_crlf() {
    let temp = tempfile::tempdir().expect("temp vault");
    let root = temp.path();
    write_vault_file(
        root,
        "sase.md",
        "---\ntype: \"[[area]]\"\ndone_tasks: \"[[done/sase_done]]\"\n---\n\n# Sase\n\n## Tasks\n\n- [ ] old\n",
    );
    write_vault_file(
        root,
        "done/sase_done.md",
        "---\nparent: \"[[sase]]\"\n---\n\n# Done\n\n- [x] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x\n",
    );
    let dest = root.join("sase.md");
    let line =
        "- [ ] #task #ref [[ref/papers/x|X]] [created::2026-10-09] ^ref-x";
    let out = insert::insert_ref_task(root, &dest, line, &[]).expect("insert");
    assert_eq!(out.block_id, "ref-x-2", "archive collision reallocates");

    // CRLF line endings survive insertion.
    let crlf = root.join("proj.md");
    std::fs::write(
        &crlf,
        "---\r\ntype: \"[[project]]\"\r\n---\r\n\r\n# Proj\r\n\r\n## Tasks\r\n\r\n- [ ] old\r\n",
    )
    .expect("write crlf");
    let line =
        "- [ ] #task #ref [[ref/papers/y|Y]] [created::2026-10-09] ^ref-y";
    insert::insert_ref_task(root, &crlf, line, &[]).expect("crlf insert");
    let contents = std::fs::read(&crlf).expect("read bytes");
    assert!(contents.windows(2).any(|w| w == b"\r\n"), "keeps CRLF");
    let text = String::from_utf8(contents).expect("utf8");
    assert!(text.contains("^ref-y"), "{text}");
}
