//! Golden tests for the read-only note catalog on a synthetic tier
//! vault: one example of every hidden tier and kind, ranking cases,
//! warnings, today handling, and vault byte-identity.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

use super::{rank, scan, NoteCatalog, NoteGroup, NoteKindKey};
use crate::native::{env::TestEnvGuard, pomodoro_homes::Hue};

/// Guarded environment for catalog scans: a fixed `BOB_NOW`, a temp
/// Bob config (valid timezone, default gmail folder), and no
/// `BOB_DAY_FILE` override. The config dir returns alive so the file
/// survives the scan.
fn scan_env(now: &str) -> (TempDir, TestEnvGuard) {
    let config_dir = tempfile::tempdir().expect("config dir");
    let config = config_dir.path().join("config.yml");
    fs::write(&config, "timezone: America/New_York\n").expect("config");
    let guard = TestEnvGuard::set(&[
        ("BOB_CONFIG_FILE", Some(config.as_os_str())),
        ("BOB_NOW", Some(OsStr::new(now))),
        ("BOB_DAY_FILE", None),
        ("BOB_HIGHLIGHTS_LIB_DIR", None),
        ("BOB_HIGHLIGHTS_XLIB_DIR", None),
        ("BOB_HIGHLIGHTS_REF_DIR", None),
    ]);
    (config_dir, guard)
}

fn write(vault: &Path, relative: &str, contents: &[u8]) {
    let path = vault.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(&path, contents).expect("write fixture");
}

fn md(vault: &Path, relative: &str, contents: &str) {
    write(vault, relative, contents.as_bytes());
}

/// The synthetic tier vault from the phase contract.
fn tier_vault() -> TempDir {
    let vault = tempfile::tempdir().expect("vault");
    let root = vault.path();
    // Hidden tiers.
    md(root, ".bob/x.md", "# x\n");
    md(root, ".sase/x.md", "# x\n");
    md(root, ".md", "# dot file\n");
    md(root, "_generated/x.md", "# x\n");
    md(root, "_meta/x.md", "# x\n");
    md(root, "_zorg_templates/x.md", "# x\n");
    md(root, "done/old.md", "# archive\n");
    md(
        root,
        "done/a_done.md",
        "---\ntitle: A\nparent: \"[[a]]\"\naliases:\n  - alpha\n---\n",
    );
    md(root, "lib/papers/p.md", "# p\n");
    write(root, "lib/papers/p.pdf", b"%PDF-1.4\n");
    md(root, "xlib/q.md", "# q\n");
    md(root, "old_lib/r.md", "# r\n");
    md(root, "ref_history/o.md", "# o\n");
    md(root, "lit_review/l.md", "# l\n");
    md(root, "ref_history/x/h.md", "# h\n");
    md(root, "notes/s.md", "# s\n");
    write(root, "notes/s.pdf", b"%PDF-1.4\n");
    md(root, "2026/20261114_done.md", "# satellite\n");
    md(root, "2026/20261114_habit.md", "# satellite\n");
    md(root, "2026/20261114_poms.md", "# satellite\n");
    md(root, "2026/20261114_events.md", "# satellite\n");
    md(root, "2026/20261113_done.md", "# satellite\n");
    md(root, "2026/20261113_habit.md", "# satellite\n");
    md(root, "2026/20261113_poms.md", "# satellite\n");
    md(root, "2026/20261113_events.md", "# satellite\n");
    write(root, "img/a.png", b"\x89PNG\r\n\x1a\n");
    // A symlinked directory is never followed.
    let outside = tempfile::tempdir().expect("outside");
    md(outside.path(), "z.md", "# z\n");
    std::os::unix::fs::symlink(outside.path(), root.join("linked"))
        .expect("symlink");
    std::mem::forget(outside);
    // Live files.
    md(root, "projects/done/keep.md", "# keep\n");
    md(root, "done.md", "# done\n");
    md(root, "2026/20261114.md", "# plain daily\n");
    md(root, "2025/20251114.md", "# plain daily\n");
    md(
        root,
        "2026/2026-W46.md",
        "---\ntype: \"[[day]]\"\n---\n\n# Week 46\n",
    );
    md(root, "2026.md", "---\ntype: \"[[day]]\"\n---\n\n# 2026\n");
    md(root, "all_time.md", "---\ntype: \"[[day]]\"\n---\n");
    md(root, "202511.md", "---\ntype: \"[[day]]\"\n---\n");
    md(root, "area.md", "---\ntype: \"[[area]]\"\n---\n");
    md(
        root,
        "life.md",
        "---\ntype: \"[[area]]\"\ncolor: green\nparent: \"[[area]]\"\n---\n",
    );
    md(
        root,
        "cash.md",
        "---\ntype: \"[[area]]\"\nparent: \"[[life]]\"\n---\n",
    );
    md(
        root,
        "deep.md",
        "---\ntype: \"[[area]]\"\nparent: \"[[cash]]\"\n---\n",
    );
    md(
        root,
        "sase.md",
        "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[life]]\"\n---\n\n- [ ] Ship it ^prj\n",
    );
    md(
        root,
        "oldproj.md",
        "---\ntype: \"[[project]]\"\nstatus: done\n---\n\n- [x] Shipped ^prj\n",
    );
    // A stale frontmatter status still closes through a checked ^prj.
    md(
        root,
        "staleproj.md",
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n\n- [x] Shipped #task ^prj\n",
    );
    // A checked ordinary task never closes the project.
    md(
        root,
        "liveproj.md",
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n\n- [x] Done task\n\n- [ ] Ship it ^prj\n",
    );
    md(
        root,
        "journal.md",
        "---\ntype: \"[[notebook]]\"\ntitle: Journal\nsummary: Daily log\n---\n",
    );
    md(root, "quill.md", "---\ntype: \"[[notebook]]\"\n---\n");
    md(
        root,
        "books.md",
        "---\ntype: \"[[album]]\"\nparent: \"[[life]]\"\nsummary: Reads\nrecord:\n  noun: book\n---\n",
    );
    md(root, "records/b1.md", "---\ntype: \"[[record]]\"\n---\n");
    md(
        root,
        "ref/papers/a.md",
        "---\ntype: \"[[ref]]\"\ntitle: Cool Paper\nstatus: read\naliases:\n  - paper\n---\n",
    );
    md(root, "essay.md", "---\ntype: \"[[ref]]\"\n---\n");
    md(
        root,
        "legacy_article.md",
        "---\ntype: \"[[article]]\"\n---\n",
    );
    md(root, "prose.md", "---\ntype: \"[[prose]]\"\n---\n");
    md(root, "mail/2026/m.md", "# m\n");
    md(root, "bank.md", "---\ntitle: Bank\n---\n");
    md(root, "bank/2026-11.md", "# November\n");
    md(root, "mac_inbox.md", "# inbox\n");
    md(root, "inbox.md", "# inbox\n");
    md(
        root,
        "ideas.md",
        "---\ntitle: Ideas\naliases:\n  - sparks\n  - flashes\n---\n",
    );
    md(root, "solo.md", "---\naliases: lone\n---\n");
    md(root, "Untitled.md", "# Untitled\n");
    md(root, "empty.md", "");
    // A `.base` view lists per the `bases` decision.
    md(root, "x.base", "---\ntitle: Base\n---\n");
    vault
}

fn scan_tier(now: &str) -> (TempDir, TempDir, TestEnvGuard, NoteCatalog) {
    let vault = tier_vault();
    let (config_dir, guard) = scan_env(now);
    let catalog = scan(vault.path());
    (vault, config_dir, guard, catalog)
}

fn by_path(catalog: &NoteCatalog) -> HashMap<&str, usize> {
    catalog
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.note_path.as_str(), index))
        .collect()
}

fn hash_vault(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&directory)
            .expect("read dir")
            .collect::<Result<_, _>>()
            .expect("entries");
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            let file_type =
                entry.path().symlink_metadata().expect("meta").file_type();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let relative =
                    path.strip_prefix(root).expect("relative").to_path_buf();
                // Unreadable files hash as absent on both sides.
                if let Ok(bytes) = fs::read(&path) {
                    files.push((relative, bytes));
                }
            }
        }
    }
    files.sort();
    files
}

fn note_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(directory).expect("read dir");
        for entry in entries {
            let entry = entry.expect("dir entry");
            let metadata = entry.path().symlink_metadata().expect("metadata");
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file()
                && entry.path().extension().is_some_and(|extension| {
                    extension == "md" || extension == "base"
                })
            {
                files.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .expect("relative")
                        .to_path_buf(),
                );
            }
        }
    }
    files.sort();
    files
}

#[test]
fn golden_listed_set_and_canonical_order() {
    let (_vault, _config, _guard, catalog) =
        scan_tier("2026-11-14T10:00:00-05:00");
    let paths: Vec<&str> = catalog
        .rows
        .iter()
        .map(|row| row.note_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "2026/20261114.md",
            "2026/2026-W46.md",
            "mac_inbox.md",
            "inbox.md",
            "life.md",
            "cash.md",
            "deep.md",
            "liveproj.md",
            "sase.md",
            "journal.md",
            "quill.md",
            "books.md",
            "area.md",
            "bank/2026-11.md",
            "bank.md",
            "done.md",
            "empty.md",
            "ideas.md",
            "projects/done/keep.md",
            "legacy_article.md",
            "mail/2026/m.md",
            "prose.md",
            "solo.md",
            "Untitled.md",
            "x.base",
            "records/b1.md",
            "oldproj.md",
            "staleproj.md",
            "all_time.md",
            "2026.md",
            "202511.md",
            "ref/papers/a.md",
            "essay.md",
            "2025/20251114.md",
        ]
    );
}

#[test]
fn shared_path_tiers_agree_with_the_living_walk() {
    let vault = tier_vault();
    let (_config, _guard) = scan_env("2026-11-14T10:00:00-05:00");
    let tiers = super::TierContext::for_vault(vault.path());
    let catalog = scan(vault.path());
    let listed: std::collections::BTreeSet<_> = catalog
        .rows
        .iter()
        .map(|row| PathBuf::from(&row.note_path))
        .collect();

    for path in note_files(vault.path()) {
        let tier = super::path_tier(vault.path(), &path, &tiers);
        if listed.contains(&path) {
            assert_eq!(tier, super::NoteTier::Living, "listed {path:?}");
        } else {
            assert!(
                matches!(
                    tier,
                    super::NoteTier::Archive | super::NoteTier::Hidden
                ),
                "skipped {path:?} classified {tier:?}"
            );
        }
    }

    for (path, expected) in [
        (".bob/x.md", super::NoteTier::Hidden),
        (".md", super::NoteTier::Hidden),
        ("_meta/x.md", super::NoteTier::Hidden),
        ("_draft.md", super::NoteTier::Living),
        ("lib/papers/p.md", super::NoteTier::Hidden),
        ("xlib/q.md", super::NoteTier::Hidden),
        ("notes/s.md", super::NoteTier::Hidden),
        ("done/a_done.md", super::NoteTier::Archive),
        ("old_lib/r.md", super::NoteTier::Archive),
        ("ref_history/x/h.md", super::NoteTier::Archive),
        ("lit_review/l.md", super::NoteTier::Archive),
        ("2026/20261114_habit.md", super::NoteTier::Archive),
        ("projects/done/keep.md", super::NoteTier::Living),
    ] {
        assert_eq!(
            super::path_tier(vault.path(), Path::new(path), &tiers),
            expected,
            "{path}"
        );
    }
}

#[test]
fn archive_scan_has_exact_rows_fields_order_and_ranking() {
    let vault = tier_vault();
    let (_config, _guard) = scan_env("2026-11-14T10:00:00-05:00");
    let catalog = super::scan_archive(vault.path());
    let paths: Vec<_> = catalog
        .rows
        .iter()
        .map(|row| row.note_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "done/a_done.md",
            "done/old.md",
            "ref_history/x/h.md",
            "lit_review/l.md",
            "ref_history/o.md",
            "old_lib/r.md",
            "2026/20261114_poms.md",
            "2026/20261114_habit.md",
            "2026/20261114_events.md",
            "2026/20261114_done.md",
            "2026/20261113_poms.md",
            "2026/20261113_habit.md",
            "2026/20261113_events.md",
            "2026/20261113_done.md",
        ]
    );
    let index = by_path(&catalog);
    let a_done = &catalog.rows[index["done/a_done.md"]];
    assert_eq!(a_done.kind, NoteKindKey::Archive);
    assert_eq!(a_done.rank, 4);
    assert_eq!(a_done.title, "A");
    assert_eq!(a_done.match_keys, vec!["A", "a_done", "alpha"]);
    assert_eq!(a_done.detail.as_deref(), Some("a"));
    assert_eq!(a_done.group, Some(NoteGroup::Done));
    assert_eq!(a_done.status, None);
    assert_eq!(a_done.hue, None);
    assert_eq!(a_done.depth, None);

    let history = &catalog.rows[index["ref_history/x/h.md"]];
    assert_eq!(history.kind, NoteKindKey::Archive);
    assert_eq!(history.title, "h");
    assert_eq!(history.detail.as_deref(), Some("ref_history/x"));
    assert_eq!(history.group, None);

    let satellite = &catalog.rows[index["2026/20261114_habit.md"]];
    assert_eq!(satellite.kind, NoteKindKey::Satellite);
    assert_eq!(satellite.rank, 8);
    assert_eq!(satellite.title, "2026-11-14 habit");
    assert_eq!(
        satellite.match_keys,
        vec!["2026-11-14 habit", "20261114_habit"]
    );
    assert_eq!(satellite.detail.as_deref(), Some("Saturday"));
    assert_eq!(satellite.group, None);
    assert_eq!(satellite.status, None);
    assert_eq!(satellite.hue, None);
    assert_eq!(satellite.depth, None);

    assert_eq!(
        super::rank(&catalog, "1114")
            .iter()
            .map(|row| row.note_path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "2026/20261114_poms.md",
            "2026/20261114_habit.md",
            "2026/20261114_events.md",
            "2026/20261114_done.md",
        ]
    );
    assert!(super::rank(&catalog, "sat").is_empty());
    assert_eq!(super::rank(&catalog, "a")[0].note_path, "done/a_done.md");
    assert!(catalog.rows.iter().all(|row| {
        !row.note_path.starts_with(".bob/")
            && !row.note_path.starts_with("lib/")
            && !row.note_path.starts_with("xlib/")
    }));
}

#[test]
fn golden_row_fields() {
    let (_vault, _config, _guard, catalog) =
        scan_tier("2026-11-14T10:00:00-05:00");
    let index = by_path(&catalog);
    let row = |path: &str| &catalog.rows[index[path]];

    let daily = row("2026/20261114.md");
    assert_eq!(daily.kind, NoteKindKey::Daily);
    assert_eq!(daily.rank, 8);
    assert_eq!(daily.title, "2026-11-14");
    assert_eq!(daily.match_keys, vec!["2026-11-14", "20261114"]);
    assert_eq!(daily.detail.as_deref(), Some("Saturday"));
    assert_eq!(daily.status, None);
    assert_eq!(daily.group, Some(NoteGroup::Today));

    let week = row("2026/2026-W46.md");
    assert_eq!(week.kind, NoteKindKey::Period);
    assert_eq!(week.rank, 6);
    assert_eq!(week.title, "2026-W46");
    assert_eq!(week.match_keys, vec!["2026-W46"]);
    assert_eq!(week.detail.as_deref(), Some("Nov 9 – 15, 2026"));
    assert_eq!(week.group, Some(NoteGroup::Today));

    let cash = row("cash.md");
    assert_eq!(cash.kind, NoteKindKey::Area);
    assert_eq!(cash.rank, 1);
    assert_eq!(cash.title, "cash");
    assert_eq!(cash.match_keys, vec!["cash"]);
    assert_eq!(cash.detail.as_deref(), Some("life"));
    assert_eq!(cash.hue, Some(Hue::Green));
    assert_eq!(cash.group, Some(NoteGroup::Areas));
    assert_eq!(cash.depth, Some(1));

    let life = row("life.md");
    assert_eq!(life.hue, Some(Hue::Green));
    assert_eq!(life.detail, None);
    assert_eq!(life.group, Some(NoteGroup::Areas));
    assert_eq!(life.depth, None);

    let deep = row("deep.md");
    assert_eq!(deep.detail.as_deref(), Some("life›cash"));
    assert_eq!(deep.depth, Some(2));

    let project = row("sase.md");
    assert_eq!(project.kind, NoteKindKey::Project);
    assert_eq!(project.rank, 2);
    assert_eq!(project.status.as_deref(), Some("WIP"));
    assert_eq!(project.hue, Some(Hue::Green));
    assert_eq!(project.detail.as_deref(), Some("life"));
    assert_eq!(project.group, Some(NoteGroup::Projects));

    let closed = row("oldproj.md");
    assert_eq!(closed.kind, NoteKindKey::Project);
    assert_eq!(closed.rank, 5);
    assert_eq!(closed.status.as_deref(), Some("Done"));
    assert_eq!(closed.group, None);

    // Stale `wip` status still closes through the checked ^prj.
    let stale = row("staleproj.md");
    assert_eq!(stale.kind, NoteKindKey::Project);
    assert_eq!(stale.rank, 5);
    assert_eq!(stale.status.as_deref(), Some("Done"));
    assert_eq!(stale.group, None);

    // A checked ordinary task leaves the project open and mapped.
    let live = row("liveproj.md");
    assert_eq!(live.kind, NoteKindKey::Project);
    assert_eq!(live.rank, 2);
    assert_eq!(live.status.as_deref(), Some("WIP"));
    assert_eq!(live.group, Some(NoteGroup::Projects));

    let notebook = row("journal.md");
    assert_eq!(notebook.kind, NoteKindKey::Notebook);
    assert_eq!(notebook.rank, 3);
    assert_eq!(notebook.detail.as_deref(), Some("Daily log"));
    assert_eq!(notebook.group, Some(NoteGroup::Notebooks));

    let album = row("books.md");
    assert_eq!(album.kind, NoteKindKey::Album);
    assert_eq!(album.rank, 3);
    assert_eq!(album.group, Some(NoteGroup::Albums));
    assert_eq!(album.hue, Some(Hue::Green));
    // Albums take the breadcrumb like any other non-notebook kind.
    assert_eq!(album.detail.as_deref(), Some("life"));

    let record = row("records/b1.md");
    assert_eq!(record.kind, NoteKindKey::Record);
    assert_eq!(record.rank, 5);

    let reference = row("ref/papers/a.md");
    assert_eq!(reference.kind, NoteKindKey::Ref);
    assert_eq!(reference.rank, 7);
    assert_eq!(reference.title, "Cool Paper");
    assert_eq!(reference.match_keys, vec!["Cool Paper", "a", "paper"]);
    assert_eq!(reference.status.as_deref(), Some("Read"));

    let mail = row("mail/2026/m.md");
    assert_eq!(mail.kind, NoteKindKey::Mail);
    assert_eq!(mail.rank, 4);

    let bank = row("bank.md");
    assert_eq!(bank.kind, NoteKindKey::Bank);
    assert_eq!(bank.rank, 4);

    let bank_month = row("bank/2026-11.md");
    assert_eq!(bank_month.kind, NoteKindKey::Bank);
    assert_eq!(bank_month.title, "2026-11");

    let ideas = row("ideas.md");
    // The stem duplicates the title case-insensitively, so it is not
    // repeated.
    assert_eq!(ideas.match_keys, vec!["Ideas", "sparks", "flashes"]);

    let solo = row("solo.md");
    assert_eq!(solo.title, "solo");
    assert_eq!(solo.match_keys, vec!["solo", "lone"]);

    let view = row("x.base");
    assert_eq!(view.kind, NoteKindKey::View);
    assert_eq!(view.rank, 4);
    assert_eq!(view.title, "x");
    assert_eq!(view.match_keys, vec!["x"]);

    let untitled = row("Untitled.md");
    assert_eq!(untitled.kind, NoteKindKey::Note);
    assert_eq!(untitled.title, "Untitled");
    assert_eq!(untitled.hue, None);

    assert_eq!(row("legacy_article.md").kind, NoteKindKey::Note);
    assert_eq!(row("prose.md").kind, NoteKindKey::Note);

    let inbox = row("mac_inbox.md");
    assert_eq!(inbox.kind, NoteKindKey::Inbox);
    assert_eq!(inbox.rank, 0);
    assert_eq!(inbox.group, Some(NoteGroup::Inbox));

    assert!(catalog.warnings.is_empty());
}

#[test]
fn ranking_cases() {
    let (_vault, _config, _guard, catalog) =
        scan_tier("2026-11-14T10:00:00-05:00");
    let paths = |rows: Vec<&super::NoteRow>| {
        rows.into_iter()
            .map(|row| row.note_path.clone())
            .collect::<Vec<_>>()
    };
    // An empty query returns the canonical snapshot.
    assert_eq!(
        paths(rank(&catalog, "")),
        catalog
            .rows
            .iter()
            .map(|row| row.note_path.clone())
            .collect::<Vec<_>>()
    );
    // The cash area outranks a ref that merely aliases cash...
    // (covered by canonical tie-break: area rank 1 before ref rank 7).
    let cash = paths(rank(&catalog, "cash"));
    assert_eq!(cash.first().map(String::as_str), Some("cash.md"));
    // Date fragments hit both dailies, newest first, without weekday
    // alias matches.
    assert_eq!(
        paths(rank(&catalog, "1114")),
        vec!["2026/20261114.md", "2025/20251114.md"]
    );
    // Weekday aliases never match dailies.
    assert!(rank(&catalog, "sat").is_empty());
    assert!(rank(&catalog, "saturday").is_empty());
    // Alias-only matches surface.
    assert_eq!(paths(rank(&catalog, "sparks")), vec!["ideas.md"]);
    // Primary matches come before path-only matches: the 2025 daily
    // never matches `2026` at all, while the year note matches
    // primarily and the mail note only through its path.
    assert_eq!(
        paths(rank(&catalog, "2026")),
        vec![
            "2026/20261114.md",
            "2026/2026-W46.md",
            "bank/2026-11.md",
            "2026.md",
            "mail/2026/m.md",
        ]
    );
    // AND across whitespace terms.
    assert_eq!(paths(rank(&catalog, "cool paper")), vec!["ref/papers/a.md"]);
    assert!(rank(&catalog, "cool zebra").is_empty());
}

#[test]
fn today_absent_without_creation() {
    let vault = tier_vault();
    let (_config, _guard) = scan_env("2020-05-05T12:00:00-04:00");
    let catalog = scan(vault.path());
    assert!(catalog
        .rows
        .iter()
        .all(|row| row.group != Some(NoteGroup::Today)));
    assert!(!vault.path().join("2020").exists());
}

#[test]
fn warnings_and_byte_identity() {
    let vault = tier_vault();
    write(vault.path(), "bad.md", &[0xff, 0xfe, b'\n']);
    write(vault.path(), "locked.md", b"# locked\n");
    let locked = vault.path().join("locked.md");
    let mut permissions = fs::metadata(&locked).expect("meta").permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&locked, permissions).expect("chmod");
    // A superuser-inherited suite still reads mode-000 files; only
    // then does the file stay listed instead of warning.
    if fs::read(&locked).is_ok() {
        fs::remove_file(&locked).expect("remove locked");
    }
    let before = hash_vault(vault.path());
    let (_config, _guard) = scan_env("2026-11-14T10:00:00-05:00");
    let catalog = scan(vault.path());
    let archive = super::scan_archive(vault.path());
    let after = hash_vault(vault.path());
    assert_eq!(before, after);
    assert!(!archive.rows.is_empty());
    assert!(catalog
        .warnings
        .iter()
        .any(|warning| warning.contains("bad.md")));
    // Restoring permissions keeps the tempdir removable everywhere.
    let _ = fs::remove_file(vault.path().join("locked.md"));
}

/// A vault reached through a symlinked path (macOS `/var` → `/private/var`
/// temp vaults, or a user `BOB_DIR` behind a symlink) must classify library
/// tiers exactly like the real path: canonical and un-canonical roots agree.
#[cfg(unix)]
#[test]
fn symlinked_vault_root_keeps_library_tiers_hidden() {
    let vault = tier_vault();
    let parent = tempfile::tempdir().expect("alias parent");
    let alias = parent.path().join("vault-alias");
    std::os::unix::fs::symlink(vault.path(), &alias).expect("symlink vault");
    let (_config, _guard) = scan_env("2026-11-14T10:00:00-05:00");
    let tiers = super::TierContext::for_vault(&alias);
    assert_eq!(
        super::path_tier(&alias, Path::new("xlib/q.md"), &tiers),
        super::NoteTier::Hidden,
        "xlib behind a symlinked vault root stays hidden"
    );
    assert_eq!(
        super::path_tier(&alias, Path::new("lib/papers/p.md"), &tiers),
        super::NoteTier::Hidden,
        "lib behind a symlinked vault root stays hidden"
    );
    let catalog = scan(&alias);
    let expected_root =
        fs::canonicalize(vault.path()).expect("canonical vault");
    assert_eq!(catalog.vault_root, expected_root);
    assert!(
        !catalog.rows.iter().any(|row| row.note_path == "xlib/q.md"),
        "symlinked scan must not list xlib/q.md"
    );
    assert!(
        catalog
            .rows
            .iter()
            .any(|row| row.note_path == "ref/papers/a.md"),
        "symlinked scan keeps ref rows"
    );
}

#[test]
fn area_typed_bank_hub_still_classifies_as_bank() {
    let (_config, _guard) = scan_env("2026-11-12T12:00:00");
    let vault = tempfile::tempdir().expect("vault");
    let root = vault.path();
    md(
        root,
        "bank.md",
        "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      kind: checking\n      start: 2026-11-01\n---\n\n# Bank\n",
    );
    let catalog = scan(root);
    let bank = catalog
        .rows
        .iter()
        .find(|row| row.note_path == "bank.md")
        .expect("bank.md row");
    assert_eq!(bank.kind, NoteKindKey::Bank);
}

/// Live-vault budget probe (ignored by default): row count, warm scan
/// time, and JSON size. Run with `BOB_DIR` set against the real vault.
#[test]
#[ignore]
fn live_vault_budget() {
    let bob_dir = crate::native::env::bob_dir();
    let started = std::time::Instant::now();
    let catalog = scan(&bob_dir);
    let elapsed = started.elapsed();
    let bytes = serde_json::to_vec(&catalog.rows).expect("json").len();
    eprintln!(
        "note_catalog: {} rows, {} warnings, {elapsed:?}, {bytes} bytes",
        catalog.rows.len(),
        catalog.warnings.len(),
    );
}
