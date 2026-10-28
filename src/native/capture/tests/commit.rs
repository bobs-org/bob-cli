//! Commit-time disk-preimage validation for the shared batch writer.
use super::*;

fn staged_existing(
    target: &Path,
    original: &str,
    updated: &str,
) -> StagedTextFile {
    StagedTextFile {
        target: target.to_path_buf(),
        target_existed: true,
        original_target: original.to_string(),
        updated_target: updated.to_string(),
    }
}

fn staged_new(target: &Path, updated: &str) -> StagedTextFile {
    StagedTextFile {
        target: target.to_path_buf(),
        target_existed: false,
        original_target: String::new(),
        updated_target: updated.to_string(),
    }
}

fn no_capture_litter(dir: &Path) {
    let litter: Vec<PathBuf> = fs::read_dir(dir)
        .expect("read fixture dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("bob-capture"))
        })
        .collect();
    assert!(litter.is_empty(), "temporary/backup litter: {litter:?}");
}

#[test]
fn refuses_when_second_note_edited_externally() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    let first_original = "- [ ] #task first\n";
    let second_original = "- [ ] #task second\n";
    fs::write(&first, first_original).expect("write first");
    fs::write(&second, second_original).expect("write second");

    let planned = vec![
        staged_existing(&first, first_original, "- [x] #task first\n"),
        staged_existing(&second, second_original, "- [x] #task second\n"),
    ];

    // External edit lands after planning.
    let external = "- [ ] #task second, edited elsewhere\n";
    fs::write(&second, external).expect("external edit");

    let error = write_staged_files(&planned).expect_err("stale batch refuses");
    assert!(
        error.message.contains("refusing to overwrite"),
        "refusal message: {}",
        error.message
    );
    assert!(
        error.message.contains("second.md"),
        "names the changed target: {}",
        error.message
    );

    // The intervening edit is preserved and the untouched note is unchanged.
    assert_eq!(fs::read_to_string(&second).expect("read second"), external);
    assert_eq!(
        fs::read_to_string(&first).expect("read first"),
        first_original
    );
    no_capture_litter(dir.path());
}

#[test]
fn refuses_when_absent_target_appears_before_commit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.md");
    let created = dir.path().join("created.md");
    let first_original = "- [ ] #task first\n";
    fs::write(&first, first_original).expect("write first");

    let planned = vec![
        staged_existing(&first, first_original, "- [x] #task first\n"),
        staged_new(&created, "- [ ] #task created\n"),
    ];

    // A new note appears at the planned path after planning.
    let intervening = "- [ ] #task someone else\n";
    fs::write(&created, intervening).expect("intervening create");

    let error = write_staged_files(&planned).expect_err("stale batch refuses");
    assert!(
        error.message.contains("refusing to overwrite"),
        "refusal message: {}",
        error.message
    );
    assert!(
        error.message.contains("created.md"),
        "names the changed target: {}",
        error.message
    );

    assert_eq!(
        fs::read_to_string(&created).expect("read created"),
        intervening
    );
    assert_eq!(
        fs::read_to_string(&first).expect("read first"),
        first_original
    );
    no_capture_litter(dir.path());
}

#[test]
fn refuses_when_existing_target_deleted_before_commit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    let first_original = "- [ ] #task first\n";
    let second_original = "- [ ] #task second\n";
    fs::write(&first, first_original).expect("write first");
    fs::write(&second, second_original).expect("write second");

    let planned = vec![
        staged_existing(&first, first_original, "- [x] #task first\n"),
        staged_existing(&second, second_original, "- [x] #task second\n"),
    ];

    fs::remove_file(&second).expect("delete second");

    let error = write_staged_files(&planned).expect_err("stale batch refuses");
    assert!(
        error.message.contains("refusing to overwrite"),
        "refusal message: {}",
        error.message
    );
    assert!(
        error.message.contains("second.md"),
        "names the changed target: {}",
        error.message
    );

    // The deleted note is not recreated and the other target is unchanged.
    assert!(!second.exists(), "deleted note stays deleted");
    assert_eq!(
        fs::read_to_string(&first).expect("read first"),
        first_original
    );
    no_capture_litter(dir.path());
}

#[test]
fn commits_matching_multi_file_preimage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    let first_original = "- [ ] #task first\n";
    let second_original = "- [ ] #task second\n";
    fs::write(&first, first_original).expect("write first");
    fs::write(&second, second_original).expect("write second");

    let first_updated = "- [x] #task first [done::2026-10-05]\n";
    let second_updated = "- [x] #task second [done::2026-10-05]\n";
    let planned = vec![
        staged_existing(&first, first_original, first_updated),
        staged_existing(&second, second_original, second_updated),
    ];

    write_staged_files(&planned).expect("matching preimage commits");

    assert_eq!(
        fs::read_to_string(&first).expect("read first"),
        first_updated
    );
    assert_eq!(
        fs::read_to_string(&second).expect("read second"),
        second_updated
    );
    no_capture_litter(dir.path());
}

#[test]
fn complete_label_batch_stale_preimage_keeps_external_edit() {
    // A complete-label-shaped batch (source, mirror target, recovered
    // dependent, daily note): mutating one planned preimage refuses the
    // whole batch, keeps the external edit, and touches nothing else.
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("a.md");
    let target = dir.path().join("b.md");
    let dependent = dir.path().join("one.md");
    let day = dir.path().join("today.md");
    let source_original = "- [ ] #task Alpha ^alpha\n";
    let target_original = "- [ ] #task Beta ^beta\n";
    let dependent_original =
        "- [?] #task Needs [dependsOn:: a__alpha] ^needs\n";
    let day_original = "## Pomodoros\n- [ ] Morning\n  - [[a#^alpha]]\n";
    for (path, contents) in [
        (&source, source_original),
        (&target, target_original),
        (&dependent, dependent_original),
        (&day, day_original),
    ] {
        fs::write(path, contents).expect("write note");
    }
    let planned = vec![
        staged_existing(
            &source,
            source_original,
            "- [x] #task Alpha  [completion:: 2026-10-14] ^alpha\n\t- 🪢 **LINKS:** implemented by [[b#^beta]]\n",
        ),
        staged_existing(
            &target,
            target_original,
            "- [ ] #task Beta ^beta\n\t- 🪢 **LINKED FROM:** [[a#^alpha]] (implemented by)\n",
        ),
        staged_existing(
            &dependent,
            dependent_original,
            "- [*] #task Needs [fresh:: 2026-10-14] [dependsOn:: a__alpha] ^needs\n",
        ),
        staged_existing(&day, day_original, "## Pomodoros\n- [ ] Morning\n  - ~~[[a#^alpha]]~~\n  - [[one#^needs]]\n"),
    ];
    let external = "- [ ] #task Beta, edited elsewhere ^beta\n";
    fs::write(&target, external).expect("external edit");
    let error = write_staged_files(&planned).expect_err("stale refuses");
    assert!(
        error.message.contains("refusing to overwrite"),
        "refusal message: {}",
        error.message
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read target"),
        external,
        "external edit is retained"
    );
    for (path, original) in [
        (&source, source_original),
        (&dependent, dependent_original),
        (&day, day_original),
    ] {
        assert_eq!(
            fs::read_to_string(path).expect("read note"),
            original,
            "{} untouched",
            path.display()
        );
    }
    no_capture_litter(dir.path());
}

#[test]
fn complete_label_batch_failure_after_replace_rolls_back() {
    // A failure after the first replacement restores source, mirror,
    // dependent, and daily-note bytes through the commit's own failure
    // path (not by calling the rollback helper directly) and cleans
    // every temporary file.
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("a.md");
    let target = dir.path().join("b.md");
    let dependent = dir.path().join("one.md");
    let day = dir.path().join("today.md");
    let source_original = "- [ ] #task Alpha ^alpha\n";
    let target_original = "- [ ] #task Beta ^beta\n";
    let dependent_original =
        "- [?] #task Needs [dependsOn:: a__alpha] ^needs\n";
    let day_original = "## Pomodoros\n- [ ] Morning\n  - [[a#^alpha]]\n";
    for (path, contents) in [
        (&source, source_original),
        (&target, target_original),
        (&dependent, dependent_original),
        (&day, day_original),
    ] {
        fs::write(path, contents).expect("write note");
    }
    let planned = vec![
        staged_existing(
            &source,
            source_original,
            "- [x] #task Alpha  [completion:: 2026-10-14] ^alpha\n",
        ),
        staged_existing(
            &target,
            target_original,
            "- [ ] #task Beta ^beta\n\t- 🪢 **LINKED FROM:** [[a#^alpha]] (implemented by)\n",
        ),
        staged_existing(
            &dependent,
            dependent_original,
            "- [*] #task Needs [fresh:: 2026-10-14] [dependsOn:: a__alpha] ^needs\n",
        ),
        staged_existing(&day, day_original, "## Pomodoros\n- [ ] Morning\n  - ~~[[a#^alpha]]~~\n"),
    ];
    *RENAME_FAIL_TARGET.lock().expect("lock seam") = Some(dependent.clone());
    let error = write_staged_files(&planned).expect_err("replace fails");
    *RENAME_FAIL_TARGET.lock().expect("lock seam") = None;
    assert!(
        error.message.contains("rolled back earlier note writes"),
        "commit invokes rollback: {}",
        error.message
    );
    for (path, original) in [
        (&source, source_original),
        (&target, target_original),
        (&dependent, dependent_original),
        (&day, day_original),
    ] {
        assert_eq!(
            fs::read_to_string(path).expect("read note"),
            original,
            "{} restored",
            path.display()
        );
    }
    no_capture_litter(dir.path());
}

#[test]
fn cumulative_stages_into_one_note_commit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let note = dir.path().join("note.md");
    let original = "- [ ] #task first\n";
    fs::write(&note, original).expect("write note");

    // Two items staged into the same note accumulate into one plan.
    let mut planner = CaptureBatchPlanner::default();
    planner
        .stage(&note, format!("{original}- [ ] #task second\n"))
        .expect("stage first item");
    let cumulative =
        format!("{original}- [ ] #task second\n- [ ] #task third\n");
    planner
        .stage(&note, cumulative.clone())
        .expect("stage second item");
    let planned = planner.into_staged_files();
    assert_eq!(planned.len(), 1, "one note stages one file");

    write_staged_files(&planned).expect("matching preimage commits");
    assert_eq!(fs::read_to_string(&note).expect("read note"), cumulative);
    no_capture_litter(dir.path());
}
