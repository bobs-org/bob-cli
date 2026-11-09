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
fn new_note_install_refuses_when_file_lands_between_check_and_install() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.md");
    let created = dir.path().join("created.md");
    let first_original = "- [ ] #task first\n";
    fs::write(&first, first_original).expect("write first");

    let planned = vec![
        staged_existing(&first, first_original, "- [x] #task first\n"),
        staged_new(&created, "- [ ] #task created\n"),
    ];
    let intervening = "- [ ] #task someone else\n";
    PLANT_BEFORE_INSTALL.with(|plant| {
        *plant.borrow_mut() = Some((created.clone(), intervening.to_string()));
    });
    let error = write_staged_files(&planned)
        .expect_err("intervening create at install refuses");
    PLANT_BEFORE_INSTALL.with(|plant| *plant.borrow_mut() = None);
    assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
    assert!(
        error.message.contains("refusing to overwrite")
            && error.message.contains("created.md")
            && error
                .message
                .contains("note was created on disk after planning"),
        "refusal message: {}",
        error.message
    );
    assert!(
        error.message.contains("rolled back earlier note writes"),
        "earlier writes roll back: {}",
        error.message
    );
    assert_eq!(
        fs::read_to_string(&created).expect("read created"),
        intervening,
        "foreign bytes survive"
    );
    assert_eq!(
        fs::read_to_string(&first).expect("read first"),
        first_original,
        "earlier edit rolls back"
    );
    no_capture_litter(dir.path());
}

#[test]
fn new_note_creates_missing_parent_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let created = dir.path().join("eat").join("ramen_nagi.md");
    let contents = "# Ramen Nagi\n";
    write_staged_files(&[staged_new(&created, contents)])
        .expect("new note creates missing parent");
    assert_eq!(
        fs::read_to_string(&created).expect("read created"),
        contents
    );
    assert!(dir.path().join("eat").is_dir());
    no_capture_litter(&dir.path().join("eat"));
}

#[test]
fn new_note_install_race_in_missing_parent_preserves_foreign_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let created = dir.path().join("eat").join("ramen_nagi.md");
    let contents = "# Ramen Nagi\n";
    let intervening = "# someone else\n";
    PLANT_BEFORE_INSTALL.with(|plant| {
        *plant.borrow_mut() = Some((created.clone(), intervening.to_string()));
    });
    let error = write_staged_files(&[staged_new(&created, contents)])
        .expect_err("intervening create at install refuses");
    PLANT_BEFORE_INSTALL.with(|plant| *plant.borrow_mut() = None);
    assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
    assert_eq!(
        fs::read_to_string(&created).expect("read created"),
        intervening,
        "foreign bytes survive"
    );
    assert!(
        dir.path().join("eat").is_dir(),
        "planted file keeps the created folder"
    );
}

#[test]
fn new_note_install_commits_missing_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let created = dir.path().join("created.md");
    let contents = "- [ ] #task created\n";
    write_staged_files(&[staged_new(&created, contents)])
        .expect("new note installs");
    assert_eq!(
        fs::read_to_string(&created).expect("read created"),
        contents
    );
    no_capture_litter(dir.path());
}

#[test]
fn new_note_install_falls_back_when_hard_links_are_unsupported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let created = dir.path().join("created.md");
    let contents = "- [ ] #task created\n";
    FORCE_HARD_LINK_UNSUPPORTED.with(|flag| flag.set(true));
    let result = write_staged_files(&[staged_new(&created, contents)]);
    FORCE_HARD_LINK_UNSUPPORTED.with(|flag| flag.set(false));
    result.expect("create_new fallback installs");
    assert_eq!(
        fs::read_to_string(&created).expect("read created"),
        contents
    );
    no_capture_litter(dir.path());
}

#[test]
fn new_note_fallback_refuses_when_file_lands_between_check_and_install() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.md");
    let created = dir.path().join("created.md");
    let first_original = "- [ ] #task first\n";
    fs::write(&first, first_original).expect("write first");
    let planned = vec![
        staged_existing(&first, first_original, "- [x] #task first\n"),
        staged_new(&created, "- [ ] #task created\n"),
    ];
    let intervening = "- [ ] #task someone else\n";
    FORCE_HARD_LINK_UNSUPPORTED.with(|flag| flag.set(true));
    PLANT_BEFORE_INSTALL.with(|plant| {
        *plant.borrow_mut() = Some((created.clone(), intervening.to_string()));
    });
    let error = write_staged_files(&planned)
        .expect_err("create_new fallback refuses an intervening file");
    FORCE_HARD_LINK_UNSUPPORTED.with(|flag| flag.set(false));
    PLANT_BEFORE_INSTALL.with(|plant| *plant.borrow_mut() = None);
    assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
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
fn existing_note_edits_still_rename_when_hard_link_fallback_is_forced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.md");
    let original = "- [ ] #task first\n";
    let updated = "- [x] #task first\n";
    fs::write(&first, original).expect("write first");
    FORCE_HARD_LINK_UNSUPPORTED.with(|flag| flag.set(true));
    let result =
        write_staged_files(&[staged_existing(&first, original, updated)]);
    FORCE_HARD_LINK_UNSUPPORTED.with(|flag| flag.set(false));
    result.expect("existing edits still commit");
    assert_eq!(fs::read_to_string(&first).expect("read first"), updated);
    no_capture_litter(dir.path());
}

#[test]
fn hard_link_unsupported_classifies_fallback_kinds() {
    assert!(hard_link_unsupported(&io::Error::from(
        io::ErrorKind::Unsupported
    )));
    assert!(hard_link_unsupported(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
    assert!(hard_link_unsupported(&io::Error::from(
        io::ErrorKind::CrossesDevices
    )));
    assert!(hard_link_unsupported(&io::Error::from_raw_os_error(
        libc::EXDEV
    )));
    assert!(hard_link_unsupported(&io::Error::from_raw_os_error(
        libc::EPERM
    )));
    assert!(!hard_link_unsupported(&io::Error::from(
        io::ErrorKind::AlreadyExists
    )));
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
    RENAME_FAIL_TARGET
        .with(|target| *target.borrow_mut() = Some(dependent.clone()));
    let error = write_staged_files(&planned).expect_err("replace fails");
    RENAME_FAIL_TARGET.with(|target| *target.borrow_mut() = None);
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
fn move_destination_is_rolled_back_when_source_replace_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("cash.md");
    let destination = dir.path().join("cash_x.md");
    let source_before = "- [ ] #task Move ^a\n";
    fs::write(&source, source_before).expect("write source");

    let mut planner = CaptureBatchPlanner::default();
    planner
        .stage(&source, "source after\n".to_string())
        .expect("stage source");
    planner
        .stage(&destination, "destination with moved task\n".to_string())
        .expect("stage destination");
    planner.record_move(&source, &destination);
    let staged = planner
        .into_staged_files()
        .expect("destination-first move order");
    assert_eq!(staged[0].target, destination);
    assert_eq!(staged[1].target, source);

    RENAME_FAIL_TARGET
        .with(|target| *target.borrow_mut() = Some(source.clone()));
    let error = write_staged_files(&staged).expect_err("source replace fails");
    RENAME_FAIL_TARGET.with(|target| *target.borrow_mut() = None);
    assert!(
        error.message.contains("rolled back earlier note writes"),
        "{error:?}"
    );
    assert_eq!(
        fs::read_to_string(&source).expect("source restored"),
        source_before
    );
    assert!(
        !destination.exists(),
        "new destination removed during rollback"
    );
    no_capture_litter(dir.path());
}

#[test]
fn move_choice_capture_rolls_back_every_file_when_source_replace_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("cash.md");
    let day = dir.path().join("day.md");
    let source_before = "---\ntype: [[area]]\n---\n- [ ] #task Alpha ^a\n";
    let day_before = "## Pomodoros\n- [ ] (1400-1500) WORK\n";
    fs::write(&source, source_before).expect("write source");
    fs::write(&day, day_before).expect("write day");
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_DAY_FILE", Some(day.as_os_str())),
        ("BOB_NOW", Some(std::ffi::OsStr::new("2026-09-20 14:31:07"))),
        ("TZ", Some(std::ffi::OsStr::new("UTC0"))),
    ]);
    let request = CaptureRequest {
        bob_dir: dir.path().to_path_buf(),
        dry_run: false,
        forced_clip: None,
        forced_destination_flags: Vec::new(),
        forced_route: None,
        forced_section: None,
        forced_sub_bullet_target: None,
        forced_drawer: None,
        inbox_routes: Vec::new(),
        move_in: Some(super::super::move_choice::MoveInPicks::Tokens(vec![
            super::super::move_choice::MoveInPick {
                sigil: ':',
                selector: "1".to_string(),
                is_number: true,
            },
        ])),
        no_clip: true,
        no_ref: true,
        pull_in: None,
        complete_offers: Vec::new(),
        cancel_offers: Vec::new(),
        offer_verdicts: Vec::new(),
        refuse_override: false,
        require_inbox_route_choice: false,
        require_pull_in_choice: false,
        expect_past_windows: None,
        argv: false,
        template_origin: None,
        raw_text: "Finish @cash^x@\n- ^^\n".to_string(),
        selection: crate::native::task_complete::SuccessorSelection::None,
    };
    let mut batch = plan_capture_batch(&request, None)
        .expect("full move-choice capture plans");
    let destination = dir.path().join("cash_x.md");
    let destination_index = batch
        .text_files
        .iter()
        .position(|file| file.target == destination)
        .expect("destination staged");
    let source_index = batch
        .text_files
        .iter()
        .position(|file| file.target == source)
        .expect("source staged");
    assert!(destination_index < source_index, "destination writes first");

    RENAME_FAIL_TARGET
        .with(|target| *target.borrow_mut() = Some(source.clone()));
    let result = commit_capture_batch(&mut batch, dir.path());
    RENAME_FAIL_TARGET.with(|target| *target.borrow_mut() = None);
    let error = result.expect_err("source replace fails");
    assert!(error.message.contains("rolled back"), "{error:?}");
    assert_eq!(
        fs::read_to_string(&source).expect("source restored"),
        source_before
    );
    assert_eq!(fs::read_to_string(&day).expect("day restored"), day_before);
    assert!(!destination.exists(), "new destination removed by rollback");
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
    let planned = planner.into_staged_files().expect("order staged files");
    assert_eq!(planned.len(), 1, "one note stages one file");

    write_staged_files(&planned).expect("matching preimage commits");
    assert_eq!(fs::read_to_string(&note).expect("read note"), cumulative);
    no_capture_litter(dir.path());
}

// KM24 (Rust equivalent of the cycler's re-plan case): a keep-moving
// pick batch (predecessor close, the picked note, and the day link) is
// refused whole when the picked note changed on disk after planning, so a
// stale pick never half-applies.
#[test]
fn keep_moving_pick_batch_stale_preimage_refuses_whole_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("sase_badges.md");
    let day = dir.path().join("today.md");
    let project_original = concat!(
        "- [ ] #task Draft the badge spec [id:: t] ^t\n",
        "- [ ] #task Write badge docs [id:: a] ^a\n",
    );
    let day_original =
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n";
    fs::write(&project, project_original).expect("write project");
    fs::write(&day, day_original).expect("write day");
    let planned = vec![
        staged_existing(
            &project,
            project_original,
            concat!(
                "- [x] #task Draft the badge spec [id:: t]  [completion:: 2026-10-05] ^t\n",
                "- [*] #task Write badge docs [fresh:: 2026-10-05] [id:: a] ^a\n",
            ),
        ),
        staged_existing(
            &day,
            day_original,
            "## Pomodoros\n- [ ] () \u{2014} FIX\n  - ~~[[sase_badges#^t]]~~\n  - [[sase_badges#^a]]\n",
        ),
    ];
    // A became Blocked elsewhere after the picker rendered.
    let external = concat!(
        "- [ ] #task Draft the badge spec [id:: t] ^t\n",
        "- [?] #task Write badge docs [dependsOn:: p] [id:: a] ^a\n",
    );
    fs::write(&project, external).expect("external edit");
    let error = write_staged_files(&planned).expect_err("stale pick refuses");
    assert!(
        error.message.contains("refusing to overwrite"),
        "refusal message: {}",
        error.message
    );
    assert_eq!(
        fs::read_to_string(&project).expect("read project"),
        external,
        "external edit is retained"
    );
    assert_eq!(
        fs::read_to_string(&day).expect("read day"),
        day_original,
        "the day file is untouched"
    );
    no_capture_litter(dir.path());
}

fn merge_fixture() -> (
    tempfile::TempDir,
    CaptureRequest,
    crate::native::env::TestEnvGuard,
) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.md"), "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Source ^s\n  - Payload\n").unwrap();
    fs::write(
        dir.path().join("z.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Target ^t\n",
    )
    .unwrap();
    fs::write(dir.path().join("readonly.md"), "Original unrelated note\n")
        .unwrap();
    let day = dir.path().join("day.md");
    let guard = crate::native::env::TestEnvGuard::set(&[
        ("BOB_DAY_FILE", Some(day.as_os_str())),
        ("BOB_NOW", Some(std::ffi::OsStr::new("2026-11-03 09:30:00"))),
    ]);
    let request = CaptureRequest {
        bob_dir: dir.path().to_path_buf(),
        dry_run: false,
        forced_clip: None,
        forced_destination_flags: Vec::new(),
        forced_route: None,
        forced_section: None,
        forced_sub_bullet_target: None,
        forced_drawer: None,
        inbox_routes: Vec::new(),
        move_in: None,
        no_clip: true,
        no_ref: true,
        pull_in: None,
        complete_offers: Vec::new(),
        cancel_offers: Vec::new(),
        offer_verdicts: Vec::new(),
        refuse_override: false,
        require_inbox_route_choice: false,
        require_pull_in_choice: false,
        expect_past_windows: None,
        argv: false,
        template_origin: None,
        raw_text: "@a+s > @z+t".to_string(),
        selection: crate::native::task_complete::SuccessorSelection::None,
    };
    (dir, request, guard)
}

#[test]
fn transfer_merge_guards_unchanged_reads_membership_and_missing_day() {
    for change in ["edit", "create", "delete", "day"] {
        let (dir, request, _env) = merge_fixture();
        let mut batch =
            plan_capture_batch(&request, None).expect("merge plans");
        let source_before = fs::read(dir.path().join("a.md")).unwrap();
        let target_before = fs::read(dir.path().join("z.md")).unwrap();
        match change {
            "edit" => {
                fs::write(dir.path().join("readonly.md"), "external edit")
                    .unwrap()
            }
            "create" => {
                fs::write(dir.path().join("new.md"), "new dependent").unwrap()
            }
            "delete" => {
                fs::remove_file(dir.path().join("readonly.md")).unwrap()
            }
            "day" => fs::write(dir.path().join("day.md"), "new daily ledger")
                .unwrap(),
            _ => unreachable!(),
        }
        let error = commit_capture_batch(&mut batch, dir.path())
            .expect_err("stale merge refuses");
        assert!(error.message.contains("changed"), "{error:?}");
        assert_eq!(fs::read(dir.path().join("a.md")).unwrap(), source_before);
        assert_eq!(fs::read(dir.path().join("z.md")).unwrap(), target_before);
        no_capture_litter(dir.path());
    }
}

#[test]
fn transfer_merge_preserves_engine_order_and_rolls_back_failed_source() {
    let (dir, request, _env) = merge_fixture();
    let mut batch = plan_capture_batch(&request, None).expect("merge plans");
    assert_eq!(
        batch.text_files.first().unwrap().target,
        dir.path().join("z.md")
    );
    assert_eq!(
        batch.text_files.last().unwrap().target,
        dir.path().join("a.md")
    );
    let source = fs::read(dir.path().join("a.md")).unwrap();
    let target = fs::read(dir.path().join("z.md")).unwrap();
    RENAME_FAIL_TARGET
        .with(|fail| *fail.borrow_mut() = Some(dir.path().join("a.md")));
    let result = commit_capture_batch(&mut batch, dir.path());
    RENAME_FAIL_TARGET.with(|fail| *fail.borrow_mut() = None);
    let error = result.expect_err("source write fails");
    assert!(error.message.contains("rolled back"));
    assert_eq!(fs::read(dir.path().join("a.md")).unwrap(), source);
    assert_eq!(fs::read(dir.path().join("z.md")).unwrap(), target);
    no_capture_litter(dir.path());
}

#[test]
fn merge_in_new_target_rolls_back_real_staged_files_on_source_replace_failure()
{
    let (dir, mut request, _env) = merge_fixture();
    request.raw_text = "New target @new^born < @a+s".to_string();
    let source_path = dir.path().join("a.md");
    let source_before = fs::read(&source_path).unwrap();
    let dependent_path = dir.path().join("dependent.md");
    let dependent_before = "---\ntype: [[area]]\n---\n## Tasks\n- [?] #task Waiting [id:: dependent] ^dependent\n  - ⛓️ **DEPENDS ON:** [[a#^s]]\n";
    fs::write(&dependent_path, dependent_before).unwrap();
    let day_path = dir.path().join("day.md");
    let day_before = "## Pomodoros\n\n- [ ] () — WORK\n  - [[a#^s]]\n";
    fs::write(&day_path, day_before).unwrap();

    let mut batch =
        plan_capture_batch(&request, None).expect("real merge-in plans");
    let new_targets: Vec<PathBuf> = batch
        .text_files
        .iter()
        .filter(|file| !file.target_existed)
        .map(|file| file.target.clone())
        .collect();
    assert_eq!(new_targets.len(), 1, "{:#?}", batch.text_files);
    let new_target = &new_targets[0];
    assert!(!new_target.exists());
    assert_eq!(
        batch.text_files.last().unwrap().target,
        source_path,
        "merge-in preserves destination-first/source-last ordering"
    );
    assert!(
        batch
            .text_files
            .iter()
            .any(|file| file.target == dependent_path),
        "the source dependent is part of this real merge plan"
    );
    assert!(
        batch.text_files.iter().any(|file| file.target == day_path),
        "the live source link is pruned in this real merge plan"
    );

    RENAME_FAIL_TARGET
        .with(|target| *target.borrow_mut() = Some(source_path.clone()));
    let error = commit_capture_batch(&mut batch, dir.path())
        .expect_err("source replacement failure rolls back earlier writes");
    RENAME_FAIL_TARGET.with(|target| *target.borrow_mut() = None);
    assert!(error.message.contains("rolled back"), "{error:?}");
    assert_eq!(fs::read(&source_path).unwrap(), source_before);
    assert_eq!(
        fs::read_to_string(&dependent_path).unwrap(),
        dependent_before
    );
    assert_eq!(fs::read_to_string(&day_path).unwrap(), day_before);
    assert!(
        !new_target.exists(),
        "rollback removes the newly created target"
    );
    no_capture_litter(dir.path());
}

#[test]
fn transfer_merge_rerun_has_no_staged_writes() {
    let (dir, request, _env) = merge_fixture();
    let mut batch = plan_capture_batch(&request, None).expect("merge plans");
    commit_capture_batch(&mut batch, dir.path()).expect("commit merge");
    let notes = super::super::batch_move::destination_snapshot(
        dir.path(),
        &CaptureBatchPlanner::default(),
    )
    .unwrap()
    .notes;
    let settings =
        crate::native::task_status_hooks::read_tasks_settings(dir.path());
    assert_eq!(
        crate::native::task_status_hooks::reconcile_merge_snapshot_for_tests(
            &notes, &settings
        ),
        notes
    );
    let again = plan_capture_batch(&request, None).expect("rerun plans");
    assert!(again.text_files.is_empty());
    assert_eq!(
        again.items[0].result.relation_merge_output.merges[0].outcome,
        crate::native::task_merge::MergeOutcome::Unchanged
    );
}

#[test]
fn transfer_merge_same_note_stages_one_file() {
    let (dir, mut request, _env) = merge_fixture();
    let source_path = dir.path().join("a.md");
    let mut source = fs::read_to_string(&source_path).unwrap();
    source.push_str("- [ ] #task Local target ^local\n");
    fs::write(&source_path, source).unwrap();
    request.raw_text = "@a+s > @a+local".to_string();
    let mut batch =
        plan_capture_batch(&request, None).expect("same-note merge");
    assert_eq!(batch.text_files.len(), 1);
    assert_eq!(batch.text_files[0].target, source_path);
    commit_capture_batch(&mut batch, dir.path())
        .expect("commit same-note merge");
    let result = fs::read_to_string(source_path).unwrap();
    assert!(result.contains("- [-] #task Source"), "{result}");
    assert!(
        result.find("^local\n").unwrap() < result.find("Payload").unwrap(),
        "{result}"
    );
}
