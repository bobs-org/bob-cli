//! Companion audio intake, late pairing, and note embed tests.
use super::*;

fn test_metadata(audio: Option<&str>) -> PipelineMetadata {
    PipelineMetadata {
        source_pdf: "lib/chat/report.pdf".to_string(),
        source_pdf_sha256: "abc123".to_string(),
        ref_type: Some("chat".to_string()),
        highlights_sidecar: None,
        highlights_count: None,
        highlights_synced_at: None,
        created: None,
        audio: audio.map(str::to_string),
    }
}

#[test]
fn companion_extensions_prefer_mp3_then_later_types() {
    assert_eq!(
        super::AUDIO_COMPANION_EXTENSIONS,
        &["mp3", "m4a", "ogg", "opus"]
    );
}

#[test]
fn plan_xlib_intake_moves_same_stem_audio_with_pdf() {
    let bob_dir = temp_bob_dir("xlib-audio-companion");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let source = config.xlib_dir.join("chat/report.pdf");
    write_test_file(&source, "pdf");
    write_test_file(&source.with_extension("mp3"), "audio");

    let moves = super::plan_xlib_intake(&config).expect("plan intake");
    assert_eq!(moves.len(), 1);
    assert_eq!(
        moves[0].companions,
        vec![(
            config.xlib_dir.join("chat/report.mp3"),
            config.lib_dir.join("chat/report.mp3"),
        )]
    );

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn plan_xlib_intake_late_pairs_orphan_audio_to_library_pdf() {
    let bob_dir = temp_bob_dir("xlib-audio-late-pair");
    let config = test_config_for_bob_dir(bob_dir.clone());
    write_test_file(&config.lib_dir.join("chat/report.pdf"), "pdf");
    write_test_file(&config.xlib_dir.join("chat/report.mp3"), "audio");

    let moves = super::plan_xlib_intake(&config).expect("plan late pair");
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].source, config.xlib_dir.join("chat/report.mp3"));
    assert_eq!(moves[0].destination, config.lib_dir.join("chat/report.mp3"));
    assert!(moves[0].companions.is_empty());

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn plan_xlib_intake_leaves_audio_without_any_pdf() {
    let bob_dir = temp_bob_dir("xlib-audio-orphan");
    let config = test_config_for_bob_dir(bob_dir.clone());
    write_test_file(&config.xlib_dir.join("chat/report.mp3"), "audio");

    let moves = super::plan_xlib_intake(&config).expect("plan orphan");
    assert!(moves.is_empty());
    let orphans = super::collect_orphan_audio(&config).expect("orphans");
    assert_eq!(orphans, vec![config.xlib_dir.join("chat/report.mp3")]);

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn plan_xlib_intake_reports_audio_destination_conflicts() {
    let bob_dir = temp_bob_dir("xlib-audio-conflict");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let source = config.xlib_dir.join("chat/report.pdf");
    write_test_file(&source, "pdf");
    write_test_file(&source.with_extension("mp3"), "new-audio");
    write_test_file(&config.lib_dir.join("chat/report.mp3"), "old-audio");

    let error = super::plan_xlib_intake(&config)
        .expect_err("audio destination conflicts must fail preflight");
    let message = error.to_string();
    assert!(
        message.contains("xlib intake collision(s) detected before writes"),
        "{message}"
    );
    assert!(
        message.contains("xlib/chat/report.mp3")
            && message.contains("lib/chat/report.mp3"),
        "{message}"
    );

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn default_note_body_embeds_audio_between_task_and_highlights() {
    let projection = test_projection(vec![
        ("status", string_value("wip")),
        ("parent", string_value("[[obsidian]]")),
        ("title", string_value("Report")),
    ]);
    let body = super::default_note_body(
        Path::new("lib/chat/report.pdf"),
        &projection,
        "lib/chat/report.pdf",
        None,
        Some("lib/chat/report.mp3"),
    );
    assert!(
        body.contains(
            "- [/] #task #ref [[lib/chat/report.pdf]] #hide ^ref\n\n![[lib/chat/report.mp3]]\n\n## Highlights\n"
        ),
        "{body}"
    );
}

#[test]
fn existing_note_inserts_embed_once_when_audio_field_is_absent() {
    let note = parse_note(
        "\
---
status: wip
parent: \"[[obsidian]]\"
---

# Report

- [ ] #task #ref [[lib/chat/report.pdf]] #hide ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
",
    );
    let metadata = test_metadata(Some("lib/chat/report.mp3"));
    let inserted =
        super::maybe_insert_audio_embed(&note, &metadata, &note.body);
    assert!(
        inserted.contains(
            "- [ ] #task #ref [[lib/chat/report.pdf]] #hide ^ref\n\n![[lib/chat/report.mp3]]\n\n## Highlights\n"
        ),
        "{inserted}"
    );
    let again = super::maybe_insert_audio_embed(&note, &metadata, &inserted);
    assert_eq!(inserted.matches("![[lib/chat/report.mp3]]").count(), 1);
    assert_eq!(again, inserted);
}

#[test]
fn existing_note_skips_embed_when_audio_field_already_present() {
    let note = parse_note(
        "\
---
status: wip
parent: \"[[obsidian]]\"
audio: \"[[lib/chat/report.mp3]]\"
---

# Report

- [ ] #task #ref [[lib/chat/report.pdf]] #hide ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
",
    );
    let metadata = test_metadata(Some("lib/chat/report.mp3"));
    let body = super::maybe_insert_audio_embed(&note, &metadata, &note.body);
    assert!(!body.contains("![[lib/chat/report.mp3]]"), "{body}");
}

#[test]
fn command_managed_audio_is_quoted_wikilink_and_omitted_without_companion() {
    let note = ParsedNote::empty();
    let mut projection = Projection::new();
    projection
        .insert("status".to_string(), MarkerValue::String("wip".to_string()));
    projection.insert(
        "parent".to_string(),
        MarkerValue::String("[[obsidian]]".to_string()),
    );
    let hash = projection_hash(&projection).expect("hash projection");
    let with_audio = note.render_with_projection(
        &projection,
        &hash,
        &test_metadata(Some("lib/chat/report.mp3")),
        "# Report\n",
    );
    assert!(
        with_audio.contains("audio: \"[[lib/chat/report.mp3]]\"\n"),
        "{with_audio}"
    );
    assert!(
        !with_audio.contains("highlights_marker_fields:"),
        "{with_audio}"
    );

    let without_audio = note.render_with_projection(
        &projection,
        &hash,
        &test_metadata(None),
        "# Report\n",
    );
    assert!(!without_audio.contains("\naudio:"), "{without_audio}");
}
