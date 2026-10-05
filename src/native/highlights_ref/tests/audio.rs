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

fn write_manifest(episode_dir: &Path, manifest: &serde_json::Value) {
    write_test_file(&episode_dir.join("manifest.json"), &manifest.to_string());
}

fn episode_manifest(
    audio_file: &str,
    source_sha: Option<&str>,
    script_sha: Option<&str>,
    created_at: &str,
) -> serde_json::Value {
    let mut source = serde_json::Map::new();
    if let Some(sha) = source_sha {
        source.insert(
            "sha256".to_string(),
            serde_json::Value::String(sha.to_string()),
        );
    }
    let mut script = serde_json::Map::new();
    if let Some(sha) = script_sha {
        script.insert(
            "sha256".to_string(),
            serde_json::Value::String(sha.to_string()),
        );
    }
    serde_json::json!({
        "created_at": created_at,
        "source": source,
        "script": script,
        "audio": {"file": audio_file},
    })
}

#[test]
fn episode_id_validation_rejects_dot_dot_slash_and_underscore() {
    assert!(super::is_valid_episode_id("report-a1b2c3"));
    assert!(super::is_valid_episode_id("REPORT.1+2*3-4"));
    assert!(!super::is_valid_episode_id(""));
    assert!(!super::is_valid_episode_id("."));
    assert!(!super::is_valid_episode_id(".."));
    assert!(!super::is_valid_episode_id("a/b"));
    assert!(!super::is_valid_episode_id("a\\b"));
    assert!(!super::is_valid_episode_id("foo_bar"));
    assert!(!super::is_valid_episode_id("foo bar"));
}

#[test]
fn frontmatter_episode_id_parses_nested_audio_key() {
    let markdown = "---\ntitle: Report\naudio:\n  episode_id: report-a1b2c3\n---\n\n# Hi\n";
    assert_eq!(
        super::frontmatter_episode_id(markdown).expect("parse episode id"),
        Some("report-a1b2c3".to_string())
    );
    assert_eq!(
        super::frontmatter_episode_id("# No frontmatter\n")
            .expect("no frontmatter"),
        None
    );
    assert_eq!(
        super::frontmatter_episode_id("---\ntitle: T\n---\n")
            .expect("no audio key"),
        None
    );
}

#[test]
fn episode_audio_source_resolves_bare_filename_only() {
    let bob_dir = temp_bob_dir("create-audio-episode");
    let library = bob_dir.join("listen-library");
    let episode_dir = library.join("report-a1b2c3");
    write_test_file(&episode_dir.join("edition.mp3"), "audio-bytes");
    write_manifest(
        &episode_dir,
        &episode_manifest(
            "edition.mp3",
            None,
            None,
            "2026-10-04T21:00:00+00:00",
        ),
    );
    let resolved = super::episode_audio_source(&library, "report-a1b2c3")
        .expect("episode audio resolves");
    assert_eq!(resolved, episode_dir.join("edition.mp3"));

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn episode_audio_source_rejects_traversal_and_missing_files() {
    let bob_dir = temp_bob_dir("create-audio-episode-traversal");
    let library = bob_dir.join("listen-library");
    for (episode_id, audio_file) in [
        ("bad-one", "../evil.mp3"),
        ("bad-two", "sub/dir.mp3"),
        ("bad-three", "/abs.mp3"),
        ("bad-four", "missing.mp3"),
        ("bad-five", "notes.txt"),
    ] {
        let episode_dir = library.join(episode_id);
        std::fs::create_dir_all(&episode_dir).expect("create episode dir");
        if audio_file == "missing.mp3" {
            write_test_file(&episode_dir.join("other.mp3"), "other");
        }
        write_manifest(
            &episode_dir,
            &episode_manifest(
                audio_file,
                None,
                None,
                "2026-10-04T21:00:00+00:00",
            ),
        );
        assert!(
            super::episode_audio_source(&library, episode_id).is_none(),
            "traversal or missing audio must not resolve: {audio_file}"
        );
    }
    assert!(super::episode_audio_source(&library, "../escape").is_none());
    assert!(super::episode_audio_source(&library, ".").is_none());

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn narration_script_candidate_strips_final_suffix() {
    assert_eq!(
        super::narration_script_candidate(Path::new("/tmp/report.md")),
        Some(PathBuf::from("/tmp/report_narration.md"))
    );
    assert_eq!(
        super::narration_script_candidate(Path::new("/tmp/report__final.md")),
        Some(PathBuf::from("/tmp/report_narration.md"))
    );
}

#[test]
fn script_hash_lookup_prefers_newest_created_at() {
    let bob_dir = temp_bob_dir("create-audio-hash-tiebreak");
    let library = bob_dir.join("listen-library");
    let digest =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    for (episode_id, created_at, via) in [
        ("old-episode", "2026-10-01T00:00:00+00:00", "source"),
        ("new-episode", "2026-10-03T00:00:00+00:00", "script"),
    ] {
        let episode_dir = library.join(episode_id);
        let audio_name = format!("{episode_id}.mp3");
        write_test_file(&episode_dir.join(&audio_name), "audio");
        let (source_sha, script_sha) = if via == "source" {
            (Some(digest), None)
        } else {
            (None, Some(digest))
        };
        write_manifest(
            &episode_dir,
            &episode_manifest(&audio_name, source_sha, script_sha, created_at),
        );
    }
    let unreadable = library.join("unreadable");
    std::fs::create_dir_all(&unreadable).expect("create unreadable dir");
    write_test_file(&unreadable.join("manifest.json"), "not json");
    let staging = library.join(".staging").join("staged");
    std::fs::create_dir_all(&staging).expect("create staging dir");
    write_test_file(&staging.join("manifest.json"), "{}");

    let found = super::find_audio_by_script_hash(&library, digest)
        .expect("hash lookup finds newest");
    assert_eq!(found, library.join("new-episode").join("new-episode.mp3"));

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn audio_library_root_prefers_env_then_config_then_xdg_then_home() {
    let prior_audio = std::env::var_os(super::ENV_AUDIO_LIBRARY);
    let prior_xdg = std::env::var_os("XDG_DATA_HOME");
    let prior_home = std::env::var_os("HOME");

    unsafe {
        std::env::set_var(super::ENV_AUDIO_LIBRARY, "/tmp/env-library");
    }
    assert_eq!(
        super::audio_library_root(Some("/tmp/config-library")),
        PathBuf::from("/tmp/env-library")
    );
    unsafe {
        std::env::remove_var(super::ENV_AUDIO_LIBRARY);
    }

    assert_eq!(
        super::audio_library_root(Some("  /tmp/config-library  ")),
        PathBuf::from("/tmp/config-library")
    );
    assert_eq!(
        super::audio_library_root(Some("")),
        super::audio_library_root(None)
    );

    unsafe {
        std::env::set_var("XDG_DATA_HOME", "/tmp/xdg-data");
    }
    assert_eq!(
        super::audio_library_root(None),
        PathBuf::from("/tmp/xdg-data/sase-listen/library")
    );

    unsafe {
        std::env::remove_var("XDG_DATA_HOME");
        std::env::set_var("HOME", "/tmp/fake-home");
    }
    assert_eq!(
        super::audio_library_root(None),
        PathBuf::from("/tmp/fake-home/.local/share/sase-listen/library")
    );

    unsafe {
        if let Some(value) = prior_audio {
            std::env::set_var(super::ENV_AUDIO_LIBRARY, value);
        } else {
            std::env::remove_var(super::ENV_AUDIO_LIBRARY);
        }
        if let Some(value) = prior_xdg {
            std::env::set_var("XDG_DATA_HOME", value);
        } else {
            std::env::remove_var("XDG_DATA_HOME");
        }
        if let Some(value) = prior_home {
            std::env::set_var("HOME", value);
        } else {
            std::env::remove_var("HOME");
        }
    }
}

#[test]
fn explicit_audio_validation_rejects_missing_and_bad_extension() {
    let bob_dir = temp_bob_dir("create-audio-explicit");
    let missing = bob_dir.join("missing.mp3");
    assert!(super::validate_explicit_audio(&missing).is_err());
    let bad = bob_dir.join("notes.txt");
    write_test_file(&bad, "text");
    assert!(super::validate_explicit_audio(&bad).is_err());
    let good = bob_dir.join("edition.MP3");
    write_test_file(&good, "audio");
    assert!(super::validate_explicit_audio(&good).is_ok());

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}
