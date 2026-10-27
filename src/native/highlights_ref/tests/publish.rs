//! Guarded stamped-PDF publication tests. Linux uses `FakeCoordinator`.
use super::super::pdf_target::{
    install_stamped_pdf, install_stamped_pdf_with, stamp_pdf_to_scratch,
};
use super::super::publish::{
    capture_destination, copy_pdf_no_clobber, publish_public_pdf,
};
use super::*;
use crate::native::config::HighlightsConfig;
use crate::native::highlights_ref::pdf_write_policy::{
    PdfWritePolicy, ReadingHoldMode,
};
use lopdf::{dictionary, Document, Object, Stream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const WIP_MARKER: &str = "- status: wip\n- parent: obsidian\n- title: Paper\n";
const NEXT_MARKER: &str =
    "- status: next\n- parent: obsidian\n- title: Paper\n";

fn coordinated_session(
    coordinator: MarkerWriteCoordinator,
) -> MarkerWriteSession {
    let mut policy = PdfWritePolicy::from_config(&HighlightsConfig::default());
    policy.coordinate_pdf_writes = true;
    MarkerWriteSession::new(
        policy.with_reading_hold(ReadingHoldMode::Exempt),
        coordinator,
    )
}

fn key_off_session(coordinator: MarkerWriteCoordinator) -> MarkerWriteSession {
    MarkerWriteSession::new(
        PdfWritePolicy::from_config(&HighlightsConfig::default())
            .with_reading_hold(ReadingHoldMode::Exempt),
        coordinator,
    )
}

fn write_pdf(path: &Path, marker: &str, pad: bool) {
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

fn pdf_bytes(marker: &str, pad: bool) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.pdf");
    write_pdf(&path, marker, pad);
    fs::read(&path).expect("read fixture PDF")
}

fn bytes_of(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read path")
}

fn audio_plan(dir: &Path, dest_pdf: &Path) -> AudioCopyPlan {
    let source = dir.join("episode.mp3");
    let dest = dest_pdf.with_extension("mp3");
    fs::write(&source, b"new-audio").expect("write source audio");
    AudioCopyPlan {
        source,
        dest,
        library_dest: None,
        origin: "--audio".to_string(),
        reused: false,
    }
}

#[test]
fn grant_runs_publication_callback_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    let stamped = dir.path().join("stamped.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    write_pdf(&stamped, NEXT_MARKER, false);
    let dest_before = bytes_of(&dest);
    let stamped_bytes = bytes_of(&stamped);
    let fake = FakeCoordinator::grant(Duration::from_millis(12));
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let calls = AtomicUsize::new(0);
    let snapshot = capture_destination(&dest).expect("snapshot");
    publish_public_pdf(&session, &dest, &snapshot, || {
        calls.fetch_add(1, Ordering::SeqCst);
        atomic_copy(&stamped, &dest)
    })
    .expect("publish");
    assert_eq!(fake.call_count(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(bytes_of(&dest), stamped_bytes);
    assert_ne!(bytes_of(&dest), dest_before);
}

#[test]
fn save_with_unchanged_marker_refuses_and_preserves_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    let saved = pdf_bytes(WIP_MARKER, true);
    assert_ne!(bytes_of(&dest), saved, "pad must change bytes");
    let fake = FakeCoordinator::grant_after_save(
        Duration::from_millis(9),
        saved.clone(),
    );
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let calls = AtomicUsize::new(0);
    let snapshot = capture_destination(&dest).expect("snapshot");
    let error = publish_public_pdf(&session, &dest, &snapshot, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect_err("unchanged-marker save");
    assert_eq!(error.code, Some("preimage_changed"));
    assert_eq!(fake.call_count(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(bytes_of(&dest), saved);
}

#[test]
fn changed_marker_refuses_and_preserves_presenter_save() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    let saved = pdf_bytes(NEXT_MARKER, false);
    let fake = FakeCoordinator::grant_after_save(
        Duration::from_millis(8),
        saved.clone(),
    );
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let calls = AtomicUsize::new(0);
    let snapshot = capture_destination(&dest).expect("snapshot");
    let error = publish_public_pdf(&session, &dest, &snapshot, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect_err("changed marker");
    assert_eq!(error.code, Some("marker_changed"));
    assert!(error.message.contains("close it in Highlights and retry"));
    assert_eq!(fake.call_count(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(bytes_of(&dest), saved);
}

#[test]
fn hold_skips_callback_and_preserves_destination_and_companion() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    let audio = dest.with_extension("mp3");
    fs::write(&audio, b"existing-audio").expect("write companion");
    let dest_before = bytes_of(&dest);
    let audio_before = bytes_of(&audio);
    let fake = FakeCoordinator::hold(Duration::from_secs(2));
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let calls = AtomicUsize::new(0);
    let snapshot = capture_destination(&dest).expect("snapshot");
    let error = publish_public_pdf(&session, &dest, &snapshot, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect_err("hold");
    assert_eq!(error.code, Some("coordination_timeout"));
    assert_eq!(fake.call_count(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(bytes_of(&dest), dest_before);
    assert_eq!(bytes_of(&audio), audio_before);
}

#[test]
fn coordinator_failure_skips_callback() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    let dest_before = bytes_of(&dest);
    let fake = FakeCoordinator::fail("presenter exploded");
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let calls = AtomicUsize::new(0);
    let snapshot = capture_destination(&dest).expect("snapshot");
    let error = publish_public_pdf(&session, &dest, &snapshot, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect_err("coordinator failure");
    assert_eq!(error.code, Some("coordination_failed"));
    assert!(error.message.contains("presenter exploded"));
    assert_eq!(fake.call_count(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(bytes_of(&dest), dest_before);
}

#[test]
fn late_destination_collision_skips_callback() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    let fake = FakeCoordinator::fail("must not run");
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let snapshot = capture_destination(&dest).expect("absent");
    assert!(snapshot.is_absent());
    write_pdf(&dest, WIP_MARKER, false);
    let occupant = bytes_of(&dest);
    let calls = AtomicUsize::new(0);
    let error = publish_public_pdf(&session, &dest, &snapshot, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect_err("late occupant");
    assert_eq!(error.code, Some("preimage_changed"));
    assert!(error.message.contains("destination appeared"));
    assert_eq!(fake.call_count(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(bytes_of(&dest), occupant);
}

#[test]
fn missing_destination_installs_without_coordination() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("xlib/papers/paper.pdf");
    let stamped = dir.path().join("stamped.pdf");
    write_pdf(&stamped, NEXT_MARKER, false);
    let fake = FakeCoordinator::fail("must not run for a new dest");
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let calls = AtomicUsize::new(0);
    let snapshot = capture_destination(&dest).expect("absent");
    publish_public_pdf(&session, &dest, &snapshot, || {
        calls.fetch_add(1, Ordering::SeqCst);
        copy_pdf_no_clobber(&stamped, &dest)
    })
    .expect("install new dest");
    assert_eq!(fake.call_count(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(bytes_of(&dest), bytes_of(&stamped));
}

#[test]
fn key_off_pass_through_does_not_call_coordinator() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    let stamped = dir.path().join("stamped.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    write_pdf(&stamped, NEXT_MARKER, false);
    let fake = FakeCoordinator::fail("key off must not coordinate");
    let session = key_off_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let snapshot = capture_destination(&dest).expect("snapshot");
    publish_public_pdf(&session, &dest, &snapshot, || {
        atomic_copy(&stamped, &dest)
    })
    .expect("key-off publish");
    assert_eq!(fake.call_count(), 0);
    assert_eq!(bytes_of(&dest), bytes_of(&stamped));
}

#[test]
fn linux_direct_pass_through_replaces_existing_dest() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    let stamped = dir.path().join("stamped.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    write_pdf(&stamped, NEXT_MARKER, false);
    let session = coordinated_session(MarkerWriteCoordinator::Direct);
    let snapshot = capture_destination(&dest).expect("snapshot");
    publish_public_pdf(&session, &dest, &snapshot, || {
        atomic_copy(&stamped, &dest)
    })
    .expect("direct publish");
    assert_eq!(bytes_of(&dest), bytes_of(&stamped));
}

#[test]
fn replacement_error_cleans_up_new_companion() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    let dest_before = bytes_of(&dest);
    let audio = audio_plan(dir.path(), &dest);
    let missing = dir.path().join("missing.pdf");
    let fake = FakeCoordinator::grant(Duration::ZERO);
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let error =
        install_stamped_pdf_with(&session, &missing, &dest, Some(&audio), 1)
            .expect_err("missing stamped PDF");
    assert!(error.message.contains("copy"));
    assert_eq!(fake.call_count(), 1);
    assert_eq!(bytes_of(&dest), dest_before);
    assert!(
        !audio.dest.exists(),
        "failed PDF install must drop new audio"
    );
}

#[test]
fn install_stamped_pdf_preserves_companion_on_changed_marker() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    let stamped = dir.path().join("stamped.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    write_pdf(&stamped, NEXT_MARKER, false);
    let audio = audio_plan(dir.path(), &dest);
    fs::write(&audio.dest, b"existing-audio").expect("existing companion");
    let saved = pdf_bytes(NEXT_MARKER, true);
    let fake = FakeCoordinator::grant_after_save(
        Duration::from_millis(5),
        saved.clone(),
    );
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let error =
        install_stamped_pdf_with(&session, &stamped, &dest, Some(&audio), 1)
            .expect_err("changed marker");
    assert_eq!(error.code, Some("marker_changed"));
    assert_eq!(fake.call_count(), 1);
    assert_eq!(bytes_of(&dest), saved);
    assert_eq!(bytes_of(&audio.dest), b"existing-audio");
}

#[test]
fn local_pdf_install_stamped_pdf_publishes_after_one_grant() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let dest = dir.path().join("xlib/papers/paper.pdf");
    write_pdf(&source, WIP_MARKER, false);
    let scratch = ScratchDir::create("publish-local-pdf").expect("scratch");
    let info = PdfInfo {
        title: Some("Paper".to_string()),
        author: None,
    };
    let (stamped, pages) =
        stamp_pdf_to_scratch(&source, &scratch, WIP_MARKER, &info)
            .expect("stamp to scratch");
    assert!(!dest.exists(), "stamping must stay private");
    let audio = audio_plan(dir.path(), &dest);
    let fake = FakeCoordinator::grant(Duration::from_millis(7));
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    let published = install_stamped_pdf_with(
        &session,
        &stamped,
        &dest,
        Some(&audio),
        pages,
    )
    .expect("install");
    assert_eq!(published, pages);
    assert_eq!(fake.call_count(), 0, "new dest does not request merging");
    assert!(dest.is_file());
    assert_eq!(bytes_of(&audio.dest), b"new-audio");
    let marker = read_pdf_marker(&dest).expect("installed marker");
    let parsed = parse_marker(&marker.contents).expect("parse");
    let expected = parse_marker(WIP_MARKER).expect("expected");
    assert_eq!(parsed, expected);
}

#[test]
fn local_pdf_existing_dest_coordinates_then_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    let dest = dir.path().join("xlib/papers/paper.pdf");
    write_pdf(&source, NEXT_MARKER, false);
    write_pdf(&dest, WIP_MARKER, false);
    let dest_before = bytes_of(&dest);
    let scratch = ScratchDir::create("publish-local-replace").expect("scratch");
    let info = PdfInfo {
        title: Some("Paper".to_string()),
        author: None,
    };
    let (stamped, pages) =
        stamp_pdf_to_scratch(&source, &scratch, NEXT_MARKER, &info)
            .expect("stamp to scratch");
    let fake = FakeCoordinator::grant(Duration::from_millis(3));
    let session =
        coordinated_session(MarkerWriteCoordinator::Fake(fake.clone()));
    install_stamped_pdf_with(&session, &stamped, &dest, None, pages)
        .expect("replace");
    assert_eq!(fake.call_count(), 1);
    assert_ne!(bytes_of(&dest), dest_before);
    let marker = read_pdf_marker(&dest).expect("installed marker");
    assert_eq!(
        parse_marker(&marker.contents).expect("parse"),
        parse_marker(NEXT_MARKER).expect("expected")
    );
}

#[test]
fn production_install_pass_through_writes_missing_dest() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    let stamped = dir.path().join("stamped.pdf");
    write_pdf(&stamped, WIP_MARKER, false);
    let pages = install_stamped_pdf(&stamped, &dest, None, 1).expect("install");
    assert_eq!(pages, 1);
    assert_eq!(bytes_of(&dest), bytes_of(&stamped));
}

#[test]
fn symlink_destination_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.pdf");
    let dest = dir.path().join("link.pdf");
    write_pdf(&real, WIP_MARKER, false);
    std::os::unix::fs::symlink(&real, &dest).expect("symlink");
    let error = capture_destination(&dest).expect_err("symlink");
    assert!(error.message.contains("symlink"));
}

#[test]
fn copy_pdf_no_clobber_refuses_an_occupant() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paper.pdf");
    let stamped = dir.path().join("stamped.pdf");
    write_pdf(&dest, WIP_MARKER, false);
    write_pdf(&stamped, NEXT_MARKER, false);
    let occupant = bytes_of(&dest);
    let error = copy_pdf_no_clobber(&stamped, &dest).expect_err("occupant");
    assert_eq!(error.code, Some("preimage_changed"));
    assert_eq!(bytes_of(&dest), occupant);
}
