//! Image-note grammar, naming, sizing, and render tests.
use super::*;

fn highlight(
    order: usize,
    text: &str,
    comment: Option<&str>,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::Highlight,
        page_label: Some("Page 1".to_string()),
        page_index: None,
        linked_page_style: false,
        text: text.to_string(),
        comment: comment.map(str::to_string),
        task_source: comment.map(str::to_string),
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn render_with(
    bob_dir: &Path,
    annotations: Vec<SidecarAnnotation>,
) -> RenderedHighlights {
    let config = test_config_for_bob_dir(bob_dir.to_path_buf());
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations,
    };
    super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &sidecar,
        false,
    )
    .expect("render image notes")
}

#[test]
fn parse_image_note_line_accepts_grammar_table() {
    let cases = [
        "~/tmp/screenshots/20261018_104022.png",
        "/tmp/screenshots/20261018_104022.png",
        "file:///tmp/Screenshot%202026.png",
        "`~/tmp/foo.png`",
        "</tmp/foo.png>",
        "\"~/tmp/foo.png\"",
        "'~/tmp/foo.png'",
        "“~/tmp/foo.png”",
        "‘~/tmp/foo.png’",
        "- ~/tmp/foo.png",
        "* ~/tmp/foo.png",
        "+ ~/tmp/foo.png",
        "Comment: ~/tmp/foo.png",
        "Note: ~/tmp/foo.png",
        "~/tmp/foo.png.",
        "~/tmp/foo\\ bar.png",
        "~/tmp/Screenshot 2026-10-18 at 10.40.22 AM.png",
        "~/tmp/foo.PNG",
    ];
    for line in cases {
        let parsed = super::parse_image_note_line(line);
        assert!(parsed.is_some(), "should accept {line:?}");
        assert!(parsed.unwrap().supported, "should be supported {line:?}");
    }

    let file_url =
        super::parse_image_note_line("file:///tmp/Screenshot%202026.png")
            .expect("file url");
    assert_eq!(file_url.source_text, "/tmp/Screenshot 2026.png");

    let escaped = super::parse_image_note_line("~/tmp/foo\\ bar.png")
        .expect("escaped spaces");
    assert_eq!(escaped.source_text, "~/tmp/foo bar.png");

    let upper = super::parse_image_note_line("~/tmp/foo.PNG").expect("upper");
    assert_eq!(upper.extension, "png");

    let tokens = super::split_image_note_lines("~/a.png ~/b.png");
    assert_eq!(tokens.0.len(), 2);
    assert_eq!(tokens.0[0].source_text, "~/a.png");
    assert_eq!(tokens.0[1].source_text, "~/b.png");
}

#[test]
fn parse_image_note_line_rejects_prose_relative_and_non_image() {
    assert!(
        super::parse_image_note_line("see ~/tmp/foo.png for the chart")
            .is_none()
    );
    assert!(super::parse_image_note_line("tmp/foo.png").is_none());
    assert!(super::parse_image_note_line("./foo.png").is_none());
    assert!(super::parse_image_note_line("~/tmp/notes.md").is_none());
}

#[test]
fn parse_image_note_line_heic_is_unsupported() {
    let parsed =
        super::parse_image_note_line("~/tmp/photo.heic").expect("heic");
    assert_eq!(parsed.extension, "heic");
    assert!(!parsed.supported);
}

#[test]
fn image_note_dest_path_is_deterministic_and_sanitized() {
    let ref_note = Path::new("/tmp/bob/ref/papers/foo.md");
    let first = super::ImageNoteRef {
        source_text: "~/tmp/screenshots/20261018_104022.png".to_string(),
        extension: "png".to_string(),
        supported: true,
    };
    let dest = super::image_note_dest_path(ref_note, &first).expect("dest");
    assert_eq!(
        dest.file_name().and_then(OsStr::to_str),
        Some("20261018_104022-07154fc6.png")
    );
    let again = super::image_note_dest_path(ref_note, &first).expect("dest");
    assert_eq!(dest, again);

    let messy = super::ImageNoteRef {
        source_text: "~/tmp/Screenshot 2026 (1).png".to_string(),
        extension: "png".to_string(),
        supported: true,
    };
    let messy_dest =
        super::image_note_dest_path(ref_note, &messy).expect("messy dest");
    let name = messy_dest
        .file_name()
        .and_then(OsStr::to_str)
        .expect("name");
    assert!(name.starts_with("Screenshot_2026_1-"), "{name}");
    assert!(name.ends_with(".png"), "{name}");

    let other = super::ImageNoteRef {
        source_text: "~/elsewhere/20261018_104022.png".to_string(),
        extension: "png".to_string(),
        supported: true,
    };
    let other_dest =
        super::image_note_dest_path(ref_note, &other).expect("other dest");
    assert_ne!(
        dest.file_name(),
        other_dest.file_name(),
        "same basename different paths must not collide"
    );
    assert!(
        !dest
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("")
            .starts_with("h-"),
        "image notes must not use TextBundle h- names"
    );
}

#[test]
fn retina_display_width_table() {
    let retina_narrow = test_png_bytes(840, 200, Some(5669));
    assert_eq!(super::retina_display_width(&retina_narrow), Some(420));

    let dpi72 = test_png_bytes(840, 200, Some(2835));
    assert_eq!(super::retina_display_width(&dpi72), None);

    let no_phys = test_png_bytes(840, 200, None);
    assert_eq!(super::retina_display_width(&no_phys), None);

    let too_wide = test_png_bytes(2880, 1664, Some(5669));
    assert_eq!(super::retina_display_width(&too_wide), None);

    assert_eq!(super::retina_display_width(b"not a png"), None);
    assert_eq!(super::retina_display_width(b"\xff\xd8\xff"), None);
}

#[test]
fn render_image_only_highlight() {
    let bob_dir = temp_bob_dir("image-note-only");
    let source = bob_dir.join("tmp/screenshots/20261018_104022.png");
    write_test_bytes(&source, &test_png_bytes(840, 200, Some(5669)));
    let rendered = render_with(
        &bob_dir,
        vec![highlight(
            0,
            "Latency is not throughput.",
            Some(source.to_str().expect("utf8")),
        )],
    );
    assert_eq!(rendered.image_note_count, 1);
    assert!(rendered.image_notes_unresolved.is_empty());
    assert_eq!(rendered.image_assets.len(), 1);
    assert!(
        rendered
            .content
            .contains("![[ref/papers/example.assets/20261018_104022-")
            && rendered.content.contains("|420]]"),
        "{}",
        rendered.content
    );
    assert!(
        !rendered.content.contains("[!note] Comment"),
        "{}",
        rendered.content
    );
}

#[test]
fn render_image_plus_comment_preserves_paragraphs() {
    let bob_dir = temp_bob_dir("image-note-comment");
    let source = bob_dir.join("shot.png");
    write_test_bytes(&source, b"png-bytes");
    let comment = format!(
        "{}\n\nFirst paragraph.\n\nSecond paragraph.",
        source.display()
    );
    let rendered = render_with(
        &bob_dir,
        vec![highlight(0, "Latency is not throughput.", Some(&comment))],
    );
    assert!(
        rendered
            .content
            .contains("> > [!note] Comment First paragraph."),
        "{}",
        rendered.content
    );
    assert!(
        rendered.content.contains("Second paragraph."),
        "{}",
        rendered.content
    );
}

#[test]
fn render_two_images_and_shared_write() {
    let bob_dir = temp_bob_dir("image-note-two");
    let source = bob_dir.join("shot.png");
    write_test_bytes(&source, b"png-bytes");
    let path = source.to_str().expect("utf8");
    let rendered = render_with(
        &bob_dir,
        vec![
            highlight(0, "First quote", Some(path)),
            highlight(1, "Second quote", Some(path)),
        ],
    );
    assert_eq!(rendered.image_note_count, 2);
    assert_eq!(rendered.image_assets.len(), 1);
    assert_eq!(
        rendered
            .content
            .matches("![[ref/papers/example.assets/")
            .count(),
        2
    );
}

#[test]
fn render_not_found_unreadable_and_unsupported() {
    let bob_dir = temp_bob_dir("image-note-states");
    let missing = bob_dir.join("missing.png");
    let dir_path = bob_dir.join("not-a-file.png");
    fs::create_dir_all(&dir_path).expect("dir at png path");
    let rendered = render_with(
        &bob_dir,
        vec![
            highlight(0, "Missing", Some(missing.to_str().expect("utf8"))),
            highlight(1, "Dir", Some(dir_path.to_str().expect("utf8"))),
            highlight(2, "Heic", Some("~/tmp/photo.heic")),
        ],
    );
    assert_eq!(rendered.image_note_count, 3);
    assert_eq!(rendered.image_notes_unresolved.len(), 3);
    assert!(
        rendered.content.contains("[!warning] Image not found"),
        "{}",
        rendered.content
    );
    assert!(
        rendered.content.contains("[!warning] Image unreadable"),
        "{}",
        rendered.content
    );
    assert!(
        rendered
            .content
            .contains("[!warning] Image type not supported"),
        "{}",
        rendered.content
    );
    assert!(rendered.image_assets.is_empty());
}

#[test]
fn render_standalone_note_image() {
    let bob_dir = temp_bob_dir("image-note-standalone");
    let source = bob_dir.join("shot.png");
    write_test_bytes(&source, b"png-bytes");
    let rendered = render_with(
        &bob_dir,
        vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::StandaloneNote,
            page_label: Some("Page 1".to_string()),
            page_index: None,
            linked_page_style: false,
            text: source.to_str().expect("utf8").to_string(),
            comment: None,
            task_source: None,
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }],
    );
    assert!(
        rendered.content.contains("> [!note]\n>\n> ![["),
        "{}",
        rendered.content
    );
}

#[test]
fn render_image_selection_plus_image_note() {
    let bob_dir = temp_bob_dir("image-note-selection");
    let sidecar_path = bob_dir.join("lib/papers/example.textbundle/text.md");
    write_test_file(
        &bob_dir.join("lib/papers/example.textbundle/assets/figure.png"),
        "synthetic image bytes",
    );
    let shot = bob_dir.join("shot.png");
    write_test_bytes(&shot, b"png-bytes");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let sidecar = SidecarInput {
        path: sidecar_path,
        annotations: vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::Image,
            page_label: Some("Page 1".to_string()),
            page_index: None,
            linked_page_style: false,
            text: "Figure".to_string(),
            comment: Some(shot.to_str().expect("utf8").to_string()),
            task_source: Some(shot.to_str().expect("utf8").to_string()),
            image: Some(SidecarImage {
                target: "assets/figure.png".to_string(),
                alt_text: Some("Figure".to_string()),
            }),
            order: 0,
            ordinal_on_page: 0,
        }],
    };
    let rendered = super::render_sidecar_highlights(
        &config,
        &pdf,
        &ref_note,
        &ParsedNote::empty(),
        &sidecar,
        false,
    )
    .expect("render selection plus note");
    assert_eq!(rendered.image_count, 1);
    assert_eq!(rendered.image_note_count, 1);
    assert_eq!(rendered.image_assets.len(), 2);
    assert!(
        rendered.content.contains("> [!quote] Image ![["),
        "{}",
        rendered.content
    );
    assert!(rendered.content.contains("shot-"), "{}", rendered.content);
}

#[test]
fn existing_copy_with_missing_source_still_embeds() {
    let bob_dir = temp_bob_dir("image-note-existing");
    let source_text = "/no/such/shot.png";
    let image_note = super::parse_image_note_line(source_text).expect("parse");
    let dest = super::image_note_dest_path(
        &bob_dir.join("ref/papers/example.md"),
        &image_note,
    )
    .expect("dest");
    write_test_bytes(&dest, b"already copied");
    let rendered =
        render_with(&bob_dir, vec![highlight(0, "Quote", Some(source_text))]);
    assert_eq!(rendered.image_note_count, 1);
    assert!(rendered.image_assets.is_empty());
    assert!(
        rendered.content.contains("![[ref/papers/example.assets/"),
        "{}",
        rendered.content
    );
    assert!(
        !rendered.content.contains("[!warning]"),
        "{}",
        rendered.content
    );
}

#[test]
fn block_ids_and_tasks_ignore_image_note_lines() {
    let bob_dir = temp_bob_dir("image-note-identity");
    let source = bob_dir.join("shot.png");
    write_test_bytes(&source, b"png-bytes");
    let without = render_with(
        &bob_dir,
        vec![highlight(
            0,
            "Latency is not throughput.",
            Some("- #task Follow up."),
        )],
    );
    let with = render_with(
        &bob_dir,
        vec![highlight(
            0,
            "Latency is not throughput.",
            Some(&format!("{}\n- #task Follow up.", source.display())),
        )],
    );
    assert_eq!(
        without.block_ids_by_annotation_order,
        with.block_ids_by_annotation_order
    );
    let config = test_config_for_bob_dir(bob_dir.clone());
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let sidecar_without = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations: vec![highlight(
            0,
            "Latency is not throughput.",
            Some("- #task Follow up."),
        )],
    };
    let sidecar_with = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations: vec![highlight(
            0,
            "Latency is not throughput.",
            Some(&format!("{}\n- #task Follow up.", source.display())),
        )],
    };
    let tasks_without = super::annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&sidecar_without),
        Some(&without),
    )
    .expect("tasks without");
    let tasks_with = super::annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&sidecar_with),
        Some(&with),
    )
    .expect("tasks with");
    assert_eq!(tasks_without.len(), 1);
    assert_eq!(tasks_with.len(), 1);
    assert_eq!(tasks_without[0].task_text, tasks_with[0].task_text);
    assert_eq!(
        tasks_without[0].source_block_id,
        tasks_with[0].source_block_id
    );
}
