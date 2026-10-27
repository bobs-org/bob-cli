//! Coordinated marker-write executor tests. Linux uses `FakeCoordinator`.
use super::*;
use crate::native::config::parse_highlights_config;
use crate::native::env::TestEnvGuard;
use crate::native::file_coordination::Coordination;
use crate::native::highlights_ref::pdf_write_policy::{
    PdfWritePolicy, ReadingHoldMode,
};
use lopdf::{dictionary, Document, Object, Stream};
use std::ffi::OsStr;
use std::time::Duration;

const WIP_MARKER: &str = "- status: wip\n- parent: obsidian\n";
const NEXT_MARKER: &str = "- status: next\n- parent: obsidian\n";
const READY_MARKER: &str = "- status: ready\n- parent: obsidian\n";
const V1_TASK: &str = "- [/] #task #ref [[lib/example.pdf]] #hide ^ref";
const V1_READY_TASK: &str = "- [ ] #task #ref [[lib/example.pdf]] #hide ^ref";

fn write_opts() -> SyncOptions {
    SyncOptions {
        dry_run: false,
        write_pdf: true,
        prefer: None,
        ignore_unannotated_sidecar: false,
    }
}

fn note_opts() -> SyncOptions {
    SyncOptions {
        dry_run: false,
        write_pdf: false,
        prefer: None,
        ignore_unannotated_sidecar: false,
    }
}

fn vault_config(bob: &Path) -> Config {
    test_config_for_bob_dir(bob.to_path_buf())
}

fn coordinated_policy() -> PdfWritePolicy {
    PdfWritePolicy::from_config(
        &parse_highlights_config(
            "highlights:\n  coordinate_pdf_writes: true\n",
            Path::new("/config.yml"),
        )
        .expect("config parses"),
    )
}

fn write_session(
    coordinator: MarkerWriteCoordinator,
    hold: ReadingHoldMode,
) -> MarkerWriteSession {
    MarkerWriteSession::new(
        coordinated_policy().with_reading_hold(hold),
        coordinator,
    )
}

fn write_pdf_with_marker(path: &Path, marker: &str, pad: bool) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create PDF parent");
    }
    let mut doc = Document::with_version("1.4");
    if pad {
        let _ = doc.add_object(dictionary! { "Dummy" => "pad" });
    }
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
    let annot_id = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(24),
            Object::Integer(24),
        ],
        "Contents" => pdf_text_string(marker),
    });
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ],
        "Contents" => content_id,
        "Annots" => vec![Object::Reference(annot_id)],
    });
    doc.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).expect("write PDF");
}

fn attach_archive_history(bob: &Path, note: &Path, archive_bytes: &[u8]) {
    let date = "2026-10-25";
    let note_rel = note
        .strip_prefix(bob)
        .expect("note is in the vault")
        .to_string_lossy()
        .replace('\\', "/");
    let live_identity = note_rel.strip_suffix(".md").expect("note extension");
    let archive_stem = "race__2026-10-25";
    let archive_rel = format!("ref_history/{archive_stem}");
    let archive_pdf_rel = format!("{archive_rel}__annotated.pdf");
    write_test_bytes(&bob.join(&archive_pdf_rel), archive_bytes);
    let hash = sha256_file(&bob.join(&archive_pdf_rel)).expect("archive hash");
    write_test_file(
        &bob.join(format!("{archive_rel}.md")),
        &format!(
            "---\nparent: \"[[{live_identity}]]\"\nannotated_pdf: \"[[{archive_pdf_rel}]]\"\nannotated_pdf_sha256: \"{hash}\"\n---\n"
        ),
    );
    let current = text_of(note);
    let (front, body) = current.split_once("\n---\n").expect("frontmatter");
    write_test_file(
        note,
        &format!(
            "{front}\nearlier:\n  - \"[[{archive_rel}|replaced {date} · 1 highlight]]\"\n---\n{body}"
        ),
    );
}

fn seed_v1(note: &Path, task_line: &str) {
    write_test_file(
        note,
        &format!(
            "{task_line}\n\n## Highlights\n\n<!-- highlights:begin -->\n\n<!-- highlights:end -->\n"
        ),
    );
}

fn seed_area(bob: &Path) {
    write_test_file(
        &bob.join("obsidian.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n",
    );
}

fn settle_v1(config: &Config, pdf: &Path) {
    let plan =
        plan_single_pdf_sync(config, pdf, note_opts()).expect("settle plan");
    execute_pdf_sync_inner(config, &plan, &mut RunWriteState::default(), None)
        .expect("settle write");
}

fn plan_checkbox_write(
    bob: &Path,
    stem: &str,
    task_line: &str,
) -> (Config, PathBuf, PathBuf, PdfSyncPlan) {
    let pdf = bob.join(format!("lib/{stem}.pdf"));
    let note = bob.join(format!("ref/{stem}.md"));
    seed_area(bob);
    write_pdf_with_marker(&pdf, WIP_MARKER, false);
    seed_v1(&note, task_line);
    let config = vault_config(bob);
    settle_v1(&config, &pdf);
    let generated = fs::read_to_string(&note).expect("read settled note");
    let updated = generated.replace("- [/] #task", "- [x] #task");
    assert_ne!(updated, generated, "checkbox should change");
    write_test_file(&note, &updated);
    let plan =
        plan_single_pdf_sync(&config, &pdf, write_opts()).expect("write plan");
    assert!(
        plan.marker_write_needed,
        "checkbox change should need a marker write"
    );
    (config, pdf, note, plan)
}

fn bytes_of(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read path")
}

fn text_of(path: &Path) -> String {
    fs::read_to_string(path).expect("read text")
}

fn write_birth_source(config: &Config, pdf: &Path) -> String {
    let source_bytes = b"# Saved source\n\nExact bytes.\n";
    let metadata = pdf_path_metadata(config, pdf).expect("PDF metadata");
    let ref_type = metadata.ref_type.expect("reference type");
    let stem = pdf.file_stem().and_then(OsStr::to_str).expect("PDF stem");
    let (_, pointer) =
        source_store::planned_pointer(&ref_type, stem, source_bytes);
    let pdf_rel = pdf
        .strip_prefix(&config.bob_dir)
        .expect("PDF is in the vault")
        .to_string_lossy()
        .replace('\\', "/");
    let catalog = code_catalog::CatalogOutcome::Missing;
    source_store::write_store(
        &config.bob_dir,
        source_store::StoreWriteParams {
            ref_type: &ref_type,
            stem,
            source_bytes,
            route: "markdown",
            origin: "fixture.md".to_string(),
            source_note: None,
            source_note_sha256: None,
            pdf_rel,
            pdf_sha256: sha256_file(pdf).expect("PDF digest"),
            catalog: &catalog,
            verified: None,
        },
    )
    .expect("write birth source store");
    pointer
}

fn assert_unchanged(
    pdf: &Path,
    note: &Path,
    pdf_before: &[u8],
    note_before: &str,
) {
    assert_eq!(bytes_of(pdf), pdf_before, "PDF should stay untouched");
    assert_eq!(text_of(note), note_before, "note should stay untouched");
}

#[test]
fn explicit_move_session_saves_first_and_notifies_forward_and_inverse() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("source.pdf");
    let to = dir.path().join("destination.pdf");
    fs::write(&from, b"disk preimage").unwrap();
    let fake = FakeCoordinator::grant_after_save(
        Duration::from_millis(25),
        b"presenter save".to_vec(),
    );
    let session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );

    let result = session
        .coordinate_explicit_pdf_move_with(&from, &to, |scope| {
            assert_eq!(fs::read(&from).unwrap(), b"presenter save");
            scope.move_forward().unwrap();
            assert!(!from.exists());
            assert_eq!(fs::read(&to).unwrap(), b"presenter save");
            scope.move_back().unwrap();
            assert!(!to.exists());
            fs::read(&from).unwrap()
        })
        .expect("move session");

    assert_eq!(
        result,
        Coordination::Granted {
            value: b"presenter save".to_vec(),
            waited: Duration::from_millis(25),
        }
    );
    assert_eq!(fake.call_count(), 1, "one move scope covers both moves");
    assert_eq!(
        fake.move_events(),
        vec![
            FakeMoveEvent::Will(from.clone(), to.clone()),
            FakeMoveEvent::Did(from.clone(), to.clone()),
            FakeMoveEvent::Will(to.clone(), from.clone()),
            FakeMoveEvent::Did(to, from),
        ]
    );
}

#[test]
fn explicit_move_hold_and_platform_error_never_run_the_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("source.pdf");
    let to = dir.path().join("destination.pdf");
    fs::write(&from, b"source").unwrap();

    for (fake, expected) in [
        (FakeCoordinator::hold(Duration::from_millis(10)), "held"),
        (FakeCoordinator::fail("permission denied"), "failed"),
    ] {
        let session = write_session(
            MarkerWriteCoordinator::Fake(fake.clone()),
            ReadingHoldMode::Exempt,
        );
        let mut ran = false;
        let result = session
            .coordinate_explicit_pdf_move_with(&from, &to, |_| {
                ran = true;
            })
            .expect("move session");
        assert!(!ran);
        match (expected, result) {
            ("held", Coordination::Held(_)) => {}
            ("failed", Coordination::Failed(message)) => {
                assert_eq!(message, "permission denied");
            }
            other => panic!("unexpected move result: {other:?}"),
        }
        assert_eq!(fake.call_count(), 1);
        assert!(fake.move_events().is_empty());
    }

    assert_eq!(fs::read(&from).unwrap(), b"source");
    assert!(!to.exists());
}

#[test]
fn explicit_move_collision_balances_notification_without_claiming_success() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("source.pdf");
    let to = dir.path().join("destination.pdf");
    fs::write(&from, b"source").unwrap();
    fs::write(&to, b"occupant").unwrap();
    let fake = FakeCoordinator::grant(Duration::ZERO);
    let session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );

    let result = session
        .coordinate_explicit_pdf_move_with(&from, &to, |scope| {
            scope.move_forward()
        })
        .expect("move session");
    let Coordination::Granted { value, .. } = result else {
        panic!("fake grant should run")
    };
    assert_eq!(value.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&from).unwrap(), b"source");
    assert_eq!(fs::read(&to).unwrap(), b"occupant");
    assert_eq!(
        fake.move_events(),
        vec![
            FakeMoveEvent::Will(from.clone(), to),
            FakeMoveEvent::Did(from.clone(), from),
        ]
    );
}

#[test]
fn fake_grant_writes_coordinated_marker_and_records_wait() {
    let bob = temp_bob_dir("marker-write-grant");
    let (config, pdf, note, plan) =
        plan_checkbox_write(&bob, "example", V1_TASK);
    let fake = FakeCoordinator::grant(Duration::from_millis(420));
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("grant execute");
    match outcome {
        PdfSyncOutcome::Written(report) => {
            let info = report.marker_write.as_ref().expect("marker write info");
            assert_eq!(info.mode, MarkerWriteMode::Coordinated);
            assert!(!info.editor_saved_first);
            assert_eq!(info.waited, Duration::from_millis(420));
            assert_eq!(
                info.status_change
                    .as_ref()
                    .map(|(from, to)| (from.as_str(), to.as_str())),
                Some(("wip", "read"))
            );
            assert_eq!(
                pdf_marker_action_display(&report),
                "updated (coordinated in 0.42 s)"
            );
        }
        other => panic!("expected write, got hold or other: {other:?}"),
    }
    assert_eq!(fake.call_count(), 1);
    let marker = read_pdf_marker(&pdf).expect("read marker");
    assert!(
        marker.contents.contains("- status: read\n"),
        "{}",
        marker.contents
    );
    assert!(
        text_of(&note).contains("status: read\n"),
        "{}",
        text_of(&note)
    );
}

#[test]
fn fake_hold_writes_nothing_for_v1() {
    let bob = temp_bob_dir("marker-write-hold");
    let (config, pdf, note, plan) =
        plan_checkbox_write(&bob, "example", V1_TASK);
    let pdf_before = bytes_of(&pdf);
    let note_before = text_of(&note);
    let fake = FakeCoordinator::hold(Duration::from_secs(2));
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("hold is success");
    match outcome {
        PdfSyncOutcome::Held(hold) => {
            assert_eq!(hold.reason, HoldReason::CoordinationTimeout);
            assert_eq!(hold.reason.as_str(), "coordination_timeout");
            assert!(
                hold.message
                    .contains("Highlights didn't answer within 10 s"),
                "{}",
                hold.message
            );
        }
        other => panic!("expected hold, got {other:?}"),
    }
    assert_eq!(fake.call_count(), 1);
    assert_unchanged(&pdf, &note, &pdf_before, &note_before);
}

#[test]
fn fake_fail_is_a_write_failure_and_writes_nothing() {
    let bob = temp_bob_dir("marker-write-fail");
    let (config, pdf, note, plan) =
        plan_checkbox_write(&bob, "example", V1_TASK);
    let pdf_before = bytes_of(&pdf);
    let note_before = text_of(&note);
    let fake = FakeCoordinator::fail("presenter exploded");
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );
    let error = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect_err("coordination failure");
    assert!(
        error.to_string().contains(
            "file coordination failed: presenter exploded; nothing was written"
        ),
        "{error}"
    );
    assert_eq!(fake.call_count(), 1);
    assert_unchanged(&pdf, &note, &pdf_before, &note_before);
}

#[test]
fn reading_hold_skips_the_coordinator_and_writes_nothing() {
    let _guard = TestEnvGuard::set(&[
        ("BOB_REF_READING_PROBE", Some(OsStr::new("reading"))),
        ("BOB_REF_HID_IDLE_SECONDS", Some(OsStr::new("1"))),
    ]);
    let bob = temp_bob_dir("marker-write-reading");
    let (config, pdf, note, plan) =
        plan_checkbox_write(&bob, "example", V1_TASK);
    let pdf_before = bytes_of(&pdf);
    let note_before = text_of(&note);
    let fake = FakeCoordinator::grant(Duration::ZERO);
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Evaluate,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("reading hold is success");
    match outcome {
        PdfSyncOutcome::Held(hold) => {
            assert_eq!(hold.reason, HoldReason::EditorActive);
            assert_eq!(
                hold.message,
                "waiting while you read in Highlights · retrying next scan"
            );
        }
        other => panic!("expected reading hold, got {other:?}"),
    }
    assert_eq!(fake.call_count(), 0);
    assert_unchanged(&pdf, &note, &pdf_before, &note_before);
}

#[test]
fn attended_and_unknown_probe_do_not_hold_for_reading() {
    let bob = temp_bob_dir("marker-write-attended");
    let (config, _pdf, _, plan) = plan_checkbox_write(&bob, "example", V1_TASK);

    let _reading = TestEnvGuard::set(&[
        ("BOB_REF_READING_PROBE", Some(OsStr::new("reading"))),
        ("BOB_REF_HID_IDLE_SECONDS", Some(OsStr::new("1"))),
    ]);
    let fake = FakeCoordinator::grant(Duration::from_millis(30));
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("attended execute");
    assert!(matches!(outcome, PdfSyncOutcome::Written(_)));
    assert_eq!(fake.call_count(), 1);
    drop(_reading);

    let _unknown = TestEnvGuard::set(&[(
        "BOB_REF_READING_PROBE",
        Some(OsStr::new("bogus")),
    )]);
    let bob2 = temp_bob_dir("marker-write-unknown");
    let (config2, _, _, plan2) = plan_checkbox_write(&bob2, "example", V1_TASK);
    let fake2 = FakeCoordinator::grant(Duration::ZERO);
    let mut session2 = write_session(
        MarkerWriteCoordinator::Fake(fake2.clone()),
        ReadingHoldMode::Evaluate,
    );
    let outcome2 = execute_pdf_sync(
        &config2,
        &plan2,
        &mut RunWriteState::default(),
        &mut session2,
    )
    .expect("unknown probe execute");
    assert!(matches!(outcome2, PdfSyncOutcome::Written(_)));
    assert_eq!(fake2.call_count(), 1);
}

#[test]
fn scan_cap_holds_later_pdfs_without_calling_the_coordinator() {
    let _guard = TestEnvGuard::set(&[
        ("BOB_REF_PDF_WRITE_BUDGET_SECONDS", Some(OsStr::new("4"))),
        ("BOB_REF_PDF_SCAN_BUDGET_SECONDS", Some(OsStr::new("4"))),
    ]);
    let bob = temp_bob_dir("marker-write-cap");
    let first_task = "- [/] #task #ref [[lib/first.pdf]] #hide ^ref";
    let second_task = "- [/] #task #ref [[lib/second.pdf]] #hide ^ref";
    let (config, first_pdf, first_note, first_plan) =
        plan_checkbox_write(&bob, "first", first_task);
    write_pdf_with_marker(&bob.join("lib/second.pdf"), WIP_MARKER, false);
    seed_v1(&bob.join("ref/second.md"), second_task);
    settle_v1(&config, &bob.join("lib/second.pdf"));
    let second_note = bob.join("ref/second.md");
    let generated = text_of(&second_note).replace("- [/] #task", "- [x] #task");
    write_test_file(&second_note, &generated);
    let second_plan = plan_single_pdf_sync(
        &config,
        &bob.join("lib/second.pdf"),
        write_opts(),
    )
    .expect("second plan");
    let second_pdf = bob.join("lib/second.pdf");
    let first_before = bytes_of(&first_pdf);
    let second_before = bytes_of(&second_pdf);
    let first_note_before = text_of(&first_note);
    let second_note_before = text_of(&second_note);

    let fake = FakeCoordinator::hold(Duration::from_secs(4));
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );
    let first = execute_pdf_sync(
        &config,
        &first_plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("first hold");
    match first {
        PdfSyncOutcome::Held(hold) => {
            assert_eq!(hold.reason, HoldReason::CoordinationTimeout);
            assert!(hold.message.contains("within 4 s"), "{}", hold.message);
        }
        other => panic!("expected per-PDF timeout, got {other:?}"),
    }
    let second = execute_pdf_sync(
        &config,
        &second_plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("cap hold");
    match second {
        PdfSyncOutcome::Held(hold) => {
            assert_eq!(hold.reason, HoldReason::CoordinationTimeout);
            assert_eq!(
                hold.message,
                "Highlights didn't answer this scan (4 s spent) · retrying next scan"
            );
        }
        other => panic!("expected scan-cap hold, got {other:?}"),
    }
    assert_eq!(fake.call_count(), 1);
    assert_eq!(bytes_of(&first_pdf), first_before);
    assert_eq!(bytes_of(&second_pdf), second_before);
    assert_eq!(text_of(&first_note), first_note_before);
    assert_eq!(text_of(&second_note), second_note_before);
}

#[test]
fn saved_first_recheck_writes_to_the_fresh_annotation_id() {
    let bob = temp_bob_dir("marker-write-saved-first");
    let (config, pdf, _, plan) = plan_checkbox_write(&bob, "example", V1_TASK);
    let planned_id = plan.marker.annotation_id;
    let planned_digest = plan.marker.source_pdf_sha256.clone();
    write_pdf_with_marker(&pdf, &plan.marker.contents, true);
    let fresh = read_pdf_marker(&pdf).expect("fresh marker");
    assert_ne!(fresh.annotation_id, planned_id);
    assert_ne!(fresh.source_pdf_sha256, planned_digest);
    assert_eq!(
        normalize_line_endings(&fresh.contents),
        normalize_line_endings(&plan.marker.contents)
    );

    let fake = FakeCoordinator::grant(Duration::from_millis(30));
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("saved-first execute");
    match outcome {
        PdfSyncOutcome::Written(report) => {
            let info = report.marker_write.as_ref().expect("marker write info");
            assert_eq!(info.mode, MarkerWriteMode::Coordinated);
            assert!(info.editor_saved_first);
            assert_eq!(
                pdf_marker_action_display(&report),
                "updated (coordinated in 0.03 s; unsaved annotations saved first)"
            );
        }
        other => panic!("expected write, got {other:?}"),
    }
    let written = read_pdf_marker(&pdf).expect("written marker");
    assert_eq!(written.annotation_id, fresh.annotation_id);
    assert!(
        written.contents.contains("- status: read\n"),
        "{}",
        written.contents
    );
}

#[test]
fn marker_changed_hold_keeps_the_editor_bytes() {
    let bob = temp_bob_dir("marker-write-changed");
    let (config, pdf, note, plan) =
        plan_checkbox_write(&bob, "example", V1_TASK);
    write_pdf_marker(&pdf, plan.marker.annotation_id, NEXT_MARKER)
        .expect("mutate marker");
    let pdf_before = bytes_of(&pdf);
    let note_before = text_of(&note);
    let fake = FakeCoordinator::grant(Duration::ZERO);
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("marker-changed hold");
    match outcome {
        PdfSyncOutcome::Held(hold) => {
            assert_eq!(hold.reason, HoldReason::MarkerChanged);
            assert_eq!(
                hold.message,
                "marker changed in Highlights during the scan · retrying next scan"
            );
        }
        other => panic!("expected marker_changed hold, got {other:?}"),
    }
    assert_eq!(fake.call_count(), 1);
    assert_unchanged(&pdf, &note, &pdf_before, &note_before);
    let marker = read_pdf_marker(&pdf).expect("read mutated marker");
    assert!(
        marker.contents.contains("- status: next\n"),
        "{}",
        marker.contents
    );
}

#[test]
fn key_off_writes_direct_and_never_calls_the_fake() {
    let bob = temp_bob_dir("marker-write-key-off");
    let (config, pdf, _, plan) = plan_checkbox_write(&bob, "example", V1_TASK);
    let fake = FakeCoordinator::fail("must not run");
    let mut session = MarkerWriteSession::new(
        PdfWritePolicy::from_config(
            &crate::native::config::HighlightsConfig::default(),
        ),
        MarkerWriteCoordinator::Fake(fake.clone()),
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("direct write");
    match outcome {
        PdfSyncOutcome::Written(report) => {
            let info = report.marker_write.as_ref().expect("marker write info");
            assert_eq!(info.mode, MarkerWriteMode::Direct);
            assert_eq!(pdf_marker_action_display(&report), "updated");
        }
        other => panic!("expected direct write, got {other:?}"),
    }
    assert_eq!(fake.call_count(), 0);
    let marker = read_pdf_marker(&pdf).expect("read marker");
    assert!(
        marker.contents.contains("- status: read\n"),
        "{}",
        marker.contents
    );
}

#[test]
fn v2_hold_leaves_the_reading_task_untouched() {
    let bob = temp_bob_dir("marker-write-v2");
    seed_area(&bob);
    let pdf = bob.join("lib/grow.pdf");
    let note = bob.join("ref/grow.md");
    let parent = bob.join("obsidian.md");
    write_pdf_with_marker(&pdf, "- status: ready\n- parent: obsidian\n", false);
    let config = vault_config(&bob);
    settle_v1(&config, &pdf);
    assert!(note.is_file(), "v2 birth should create the ref note");
    let parent_after_birth = text_of(&parent);
    assert!(
        parent_after_birth.contains("#task #ref [[ref/grow|"),
        "{parent_after_birth}"
    );
    let before = text_of(&note);
    let moved = before.replace("status: ready", "status: wip");
    assert_ne!(moved, before);
    write_test_file(&note, &moved);
    let plan = plan_single_pdf_sync(&config, &pdf, write_opts())
        .expect("v2 write plan");
    assert!(plan.marker_write_needed);
    assert!(
        plan.reading_task_plan.is_some(),
        "v2 status change should plan a reading-task edit"
    );
    let pdf_before = bytes_of(&pdf);
    let note_before = text_of(&note);
    let parent_before = text_of(&parent);
    let fake = FakeCoordinator::hold(Duration::from_secs(1));
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("v2 hold");
    assert!(matches!(outcome, PdfSyncOutcome::Held(_)));
    assert_eq!(bytes_of(&pdf), pdf_before);
    assert_eq!(text_of(&note), note_before);
    assert_eq!(text_of(&parent), parent_before);
}

#[test]
fn v1_execution_refresh_carries_planned_source_pointer_through_marker_write() {
    let bob = temp_bob_dir("marker-write-v1-source-pointer");
    let (config, pdf, note, mut plan) =
        plan_checkbox_write(&bob, "example", V1_TASK);
    let pointer =
        ".bob/ref-sources/chat/example--0123456789ab/source.md".to_string();
    plan.stable_metadata.source_markdown = Some(pointer.clone());

    let fake = FakeCoordinator::grant(Duration::ZERO);
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("execute v1 marker write");
    assert!(matches!(outcome, PdfSyncOutcome::Written(_)));

    let persisted = frontmatter::read_note(&note).expect("read v1 note");
    assert_eq!(
        persisted
            .frontmatter_value(FIELD_SOURCE_MARKDOWN)
            .and_then(|value| value.as_string().map(str::to_string))
            .as_deref(),
        Some(pointer.as_str())
    );
    assert_eq!(
        text_of(&note).matches("source_markdown:").count(),
        1,
        "v1 execution must not duplicate the pointer"
    );
    assert_eq!(
        persisted
            .frontmatter_value(FIELD_SOURCE_PDF_SHA256)
            .and_then(|value| value.as_string().map(str::to_string))
            .as_deref(),
        Some(sha256_file(&pdf).expect("post-write PDF digest").as_str()),
        "marker rewrite must retain the post-write PDF digest"
    );
}

#[test]
fn v2_birth_attaches_verified_source_through_normalizing_marker_write() {
    let bob = temp_bob_dir("marker-write-v2-source-pointer");
    seed_area(&bob);
    let pdf = bob.join("lib/chat/grow.pdf");
    let note = bob.join("ref/chat/grow.md");
    write_pdf_with_marker(
        &pdf,
        "- status: unread\n- parent: obsidian\n",
        false,
    );
    let config = vault_config(&bob);
    let pointer = write_birth_source(&config, &pdf);
    let plan = plan_single_pdf_sync(&config, &pdf, write_opts())
        .expect("plan v2 birth with marker normalization");
    assert!(plan.marker_write_needed, "deprecated marker needs rewrite");
    assert_eq!(
        plan.stable_metadata.source_markdown.as_deref(),
        Some(pointer.as_str()),
        "new-note planning must derive the verified birth pointer"
    );
    let planned_pdf_digest = plan.marker.source_pdf_sha256.clone();

    let fake = FakeCoordinator::grant(Duration::ZERO);
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("execute v2 birth marker write");
    assert!(matches!(outcome, PdfSyncOutcome::Written(_)));
    assert_ne!(
        sha256_file(&pdf).expect("post-write PDF digest"),
        planned_pdf_digest,
        "marker rewrite should change the PDF bytes"
    );

    let persisted = frontmatter::read_note(&note).expect("read v2 note");
    assert_eq!(
        persisted
            .frontmatter_value(FIELD_SOURCE_MARKDOWN)
            .and_then(|value| value.as_string().map(str::to_string))
            .as_deref(),
        Some(pointer.as_str()),
        "execution must carry the pointer planned against the original PDF"
    );
    assert_eq!(
        persisted
            .frontmatter_value(FIELD_SOURCE_PDF_SHA256)
            .and_then(|value| value.as_string().map(str::to_string))
            .as_deref(),
        Some(sha256_file(&pdf).expect("post-write PDF digest").as_str()),
        "note must record the post-write PDF digest"
    );
}

#[test]
fn v2_birth_does_not_attach_a_source_that_fails_digest_verification() {
    let bob = temp_bob_dir("marker-write-v2-corrupt-source");
    seed_area(&bob);
    let pdf = bob.join("lib/chat/corrupt.pdf");
    write_pdf_with_marker(&pdf, READY_MARKER, false);
    let config = vault_config(&bob);
    let pointer = write_birth_source(&config, &pdf);
    write_test_bytes(&bob.join(&pointer), b"corrupted saved source");

    let plan = plan_single_pdf_sync(&config, &pdf, write_opts())
        .expect("plan birth with corrupt source store");
    assert_eq!(
        plan.stable_metadata.source_markdown, None,
        "a catalog match cannot attach source bytes that fail verification"
    );
}

fn plan_v1_stale_restore(
    bob: &Path,
    stem: &str,
) -> (Config, PathBuf, PathBuf, PdfSyncPlan) {
    let pdf = bob.join(format!("lib/{stem}.pdf"));
    let note = bob.join(format!("ref/{stem}.md"));
    seed_area(bob);
    write_pdf_with_marker(&pdf, READY_MARKER, false);
    seed_v1(&note, &V1_READY_TASK.replace("example", stem));
    let config = vault_config(bob);
    settle_v1(&config, &pdf);
    let generated = text_of(&note);
    let updated = generated.replace("- [ ] #task", "- [*] #task");
    assert_ne!(updated, generated, "checkbox should change");
    write_test_file(&note, &updated);
    let write_plan = plan_single_pdf_sync(&config, &pdf, write_opts())
        .expect("next-marker write plan");
    assert!(write_plan.marker_write_needed);
    execute_pdf_sync_inner(
        &config,
        &write_plan,
        &mut RunWriteState::default(),
        None,
    )
    .expect("write next marker");
    let current = read_pdf_marker(&pdf).expect("read next marker");
    write_pdf_marker(&pdf, current.annotation_id, READY_MARKER)
        .expect("revert marker to the replaced copy");
    let plan = plan_single_pdf_sync(&config, &pdf, write_opts())
        .expect("stale restore plan");
    assert!(plan.stale_restore, "stale save should restore");
    assert!(plan.marker_write_needed, "restore needs a marker write");
    (config, pdf, note, plan)
}

#[test]
fn coordinated_saved_first_restores_a_stale_marker_inside_the_accessor() {
    let bob = temp_bob_dir("marker-write-stale-restore");
    let (config, pdf, note, plan) = plan_v1_stale_restore(&bob, "example");
    write_pdf_with_marker(&pdf, &plan.marker.contents, true);
    let fake = FakeCoordinator::grant(Duration::from_millis(30));
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake),
        ReadingHoldMode::Exempt,
    );
    let outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("stale restore execute");
    match outcome {
        PdfSyncOutcome::Written(report) => {
            let info = report.marker_write.as_ref().expect("marker write info");
            assert_eq!(info.mode, MarkerWriteMode::Coordinated);
            assert!(info.editor_saved_first);
            assert!(info.restored);
        }
        other => panic!("expected restore write, got {other:?}"),
    }
    let written = read_pdf_marker(&pdf).expect("restored marker");
    assert!(
        written.contents.contains("- status: next\n"),
        "{}",
        written.contents
    );
    let note_after = text_of(&note);
    assert!(note_after.contains("status: next\n"), "{note_after}");
    assert!(note_after.contains("- [*] #task #ref"), "{note_after}");
    assert!(
        note_after.contains("highlights_marker_replaced_hash:"),
        "{note_after}"
    );
}

#[test]
fn same_digest_recheck_keeps_the_planned_annotation_id() {
    let bob = temp_bob_dir("marker-write-same-digest");
    let (config, pdf, _, plan) = plan_checkbox_write(&bob, "example", V1_TASK);
    let planned_id = plan.marker.annotation_id;
    let fake = FakeCoordinator::grant(Duration::ZERO);
    let mut session = write_session(
        MarkerWriteCoordinator::Fake(fake),
        ReadingHoldMode::Exempt,
    );
    execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    )
    .expect("same-digest write");
    let written = read_pdf_marker(&pdf).expect("written marker");
    assert_eq!(written.annotation_id, planned_id);
    assert!(written.contents.contains("- status: read\n"));
}

#[test]
fn reading_hold_report_names_attended_holding_and_unknown() {
    let bob = temp_bob_dir("marker-write-report");
    let (_, _, _, plan) = plan_checkbox_write(&bob, "example", V1_TASK);
    let plans = [&plan];

    let attended =
        write_session(MarkerWriteCoordinator::Direct, ReadingHoldMode::Exempt);
    assert_eq!(attended.reading_hold_report(&plans, false), "attended");
    assert_eq!(attended.reading_hold_report(&plans, true), "n/a");

    let _reading = TestEnvGuard::set(&[
        ("BOB_REF_READING_PROBE", Some(OsStr::new("reading"))),
        ("BOB_REF_HID_IDLE_SECONDS", Some(OsStr::new("4"))),
    ]);
    let holding = write_session(
        MarkerWriteCoordinator::Direct,
        ReadingHoldMode::Evaluate,
    );
    assert_eq!(
        holding.reading_hold_report(&plans, false),
        "holding (Highlights frontmost, last input 4 s ago)"
    );
    drop(_reading);

    let _away = TestEnvGuard::set(&[(
        "BOB_REF_READING_PROBE",
        Some(OsStr::new("away")),
    )]);
    let clear = write_session(
        MarkerWriteCoordinator::Direct,
        ReadingHoldMode::Evaluate,
    );
    assert_eq!(clear.reading_hold_report(&plans, false), "clear");
}

#[test]
fn restored_capture_is_held_at_execution_and_after_coordinated_save() {
    let bob = temp_bob_dir("marker-write-replaced-capture");
    seed_area(&bob);
    let config = vault_config(&bob);
    let pdf = bob.join("lib/race.pdf");
    let note = bob.join("ref/race.md");
    write_pdf_with_marker(&pdf, WIP_MARKER, false);
    seed_v1(&note, &V1_TASK.replace("example", "race"));
    settle_v1(&config, &pdf);

    let archived_source = bob.join("archived-source.pdf");
    write_pdf_with_marker(&archived_source, NEXT_MARKER, true);
    let archived_bytes = bytes_of(&archived_source);
    let current_note = text_of(&note);
    let completed = current_note.replace("- [/] #task", "- [x] #task");
    assert_ne!(completed, current_note);
    write_test_file(&note, &completed);
    attach_archive_history(&bob, &note, &archived_bytes);

    let plan = plan_single_pdf_sync(&config, &pdf, write_opts())
        .expect("plan against current replacement PDF");
    assert!(plan.marker_write_needed, "fixture also needs marker repair");
    let note_before = text_of(&note);
    let replacement_bytes = bytes_of(&pdf);

    // A presenter restore between planning and execution is held before the
    // marker or any other per-PDF write path starts.
    write_test_bytes(&pdf, &archived_bytes);
    let mut direct = MarkerWriteSession::uncoordinated();
    let direct_outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut direct,
    )
    .expect("race recheck");
    match direct_outcome {
        PdfSyncOutcome::Held(hold) => {
            assert_eq!(hold.reason, HoldReason::ReplacedCapture);
            assert_eq!(
                hold.message,
                "Highlights wrote the replaced capture back"
            );
        }
        other => panic!("expected replaced-capture hold, got {other:?}"),
    }
    assert_eq!(bytes_of(&pdf), archived_bytes);
    assert_eq!(text_of(&note), note_before);

    // Simulate Highlights saving the old capture when coordination is
    // granted. The callback must recheck its digest before marker repair.
    write_test_bytes(&pdf, &replacement_bytes);
    let fake = FakeCoordinator::grant_after_save(
        Duration::from_millis(10),
        archived_bytes.clone(),
    );
    let mut coordinated = write_session(
        MarkerWriteCoordinator::Fake(fake.clone()),
        ReadingHoldMode::Exempt,
    );
    let coordinated_outcome = execute_pdf_sync(
        &config,
        &plan,
        &mut RunWriteState::default(),
        &mut coordinated,
    )
    .expect("coordinated save recheck");
    match coordinated_outcome {
        PdfSyncOutcome::Held(hold) => {
            assert_eq!(hold.reason, HoldReason::ReplacedCapture);
            assert_eq!(
                hold.message,
                "Highlights wrote the replaced capture back"
            );
        }
        other => panic!("expected replaced-capture hold, got {other:?}"),
    }
    assert_eq!(fake.call_count(), 1);
    assert_eq!(bytes_of(&pdf), archived_bytes);
    assert_eq!(text_of(&note), note_before);
}
