//! Config, projection, merge, and path metadata tests.
use super::*;

#[test]
fn relative_config_paths_resolve_under_bob_dir() {
    let bob_dir = Path::new("/tmp/bob");

    assert_eq!(
        resolve_under_bob(bob_dir, Path::new("library")),
        PathBuf::from("/tmp/bob/library")
    );
    assert_eq!(
        resolve_under_bob(bob_dir, Path::new("/var/lib/pdfs")),
        PathBuf::from("/var/lib/pdfs")
    );
}

#[test]
fn plan_xlib_intake_maps_nested_paths_and_companions() {
    let bob_dir = temp_bob_dir("xlib-intake");
    let config = test_config_for_bob_dir(bob_dir.clone());

    let missing = super::plan_xlib_intake(&config).expect("missing xlib is ok");
    assert!(missing.is_empty());

    let source = config.xlib_dir.join("chat/a/b.pdf");
    let markdown = source.with_extension("md");
    let textbundle = source.with_extension("textbundle");
    write_test_file(&source, "pdf");
    write_test_file(&markdown, "# Sidecar\n");
    write_test_file(&textbundle.join("text.md"), "# TextBundle\n");

    let moves = super::plan_xlib_intake(&config).expect("plan intake");

    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].source, source);
    assert_eq!(moves[0].destination, config.lib_dir.join("chat/a/b.pdf"));
    assert_eq!(
        moves[0].companions,
        vec![
            (
                config.xlib_dir.join("chat/a/b.md"),
                config.lib_dir.join("chat/a/b.md"),
            ),
            (
                config.xlib_dir.join("chat/a/b.textbundle"),
                config.lib_dir.join("chat/a/b.textbundle"),
            ),
        ]
    );

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn plan_xlib_intake_reports_every_destination_conflict() {
    let bob_dir = temp_bob_dir("xlib-intake-conflicts");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let source = config.xlib_dir.join("chat/report.pdf");
    write_test_file(&source, "pdf");
    write_test_file(&source.with_extension("md"), "# Sidecar\n");
    write_test_file(
        &source.with_extension("textbundle").join("text.md"),
        "# Bundle\n",
    );
    write_test_file(&config.lib_dir.join("chat/report.pdf"), "archived");
    write_test_file(&config.lib_dir.join("chat/report.md"), "# Archived\n");
    write_test_file(
        &config.lib_dir.join("chat/report.textbundle/text.md"),
        "# Archived bundle\n",
    );

    let error = super::plan_xlib_intake(&config)
        .expect_err("destination conflicts must fail preflight");

    let message = error.to_string();
    assert!(
        message.contains("xlib intake collision(s) detected before writes"),
        "{message}"
    );
    assert!(
        message.contains("xlib/chat/report.pdf")
            && message.contains("lib/chat/report.pdf"),
        "{message}"
    );
    assert!(
        message.contains("xlib/chat/report.md")
            && message.contains("lib/chat/report.md"),
        "{message}"
    );
    assert!(
        message.contains("xlib/chat/report.textbundle")
            && message.contains("lib/chat/report.textbundle"),
        "{message}"
    );

    fs::remove_dir_all(bob_dir).expect("remove temp bob dir");
}

#[test]
fn validate_library_layout_rejects_equal_and_nested_paths() {
    let bob_dir = PathBuf::from("/tmp/bob");
    let mut config = test_config_for_bob_dir(bob_dir.clone());
    assert!(super::validate_library_layout(&config).is_ok());

    config.xlib_dir = config.lib_dir.clone();
    assert!(super::validate_library_layout(&config).is_err());

    config.xlib_dir = bob_dir.join("lib/intake");
    assert!(super::validate_library_layout(&config).is_err());

    config.lib_dir = bob_dir.join("xlib/archive");
    config.xlib_dir = bob_dir.join("xlib");
    assert!(super::validate_library_layout(&config).is_err());
}

#[test]
fn pipeline_fields_exclude_marker_user_projection() {
    assert!(super::PIPELINE_FIELDS.contains(&"source_pdf"));
    assert!(super::PIPELINE_FIELDS.contains(&"highlights_marker_hash"));
    assert!(super::PIPELINE_FIELDS.contains(&"highlights_marker_base"));
    assert!(super::PIPELINE_FIELDS.contains(&"highlights_marker_replaced_hash"));
    let base = super::PIPELINE_FIELDS
        .iter()
        .position(|field| *field == "highlights_marker_base")
        .expect("base field");
    let replaced = super::PIPELINE_FIELDS
        .iter()
        .position(|field| *field == "highlights_marker_replaced_hash")
        .expect("replaced-hash field");
    assert_eq!(replaced, base + 1);
    assert!(!super::PIPELINE_FIELDS.contains(&"status"));
    assert!(!super::PIPELINE_FIELDS.contains(&"parent"));
    assert!(!super::PIPELINE_FIELDS.contains(&"type"));
    assert!(super::is_command_managed_field("type"));
    assert!(super::is_command_managed_field("ref_type"));
    assert!(super::is_command_managed_field("audio"));
    assert!(!super::PIPELINE_FIELDS.contains(&"audio"));
}

#[test]
fn projection_snapshot_json_round_trips_compact_user_projection() {
    let projection = test_projection(vec![
        ("status", string_value("wip")),
        ("parent", string_value("[[obsidian]]")),
        ("title", string_value("Systems Performance")),
        (
            "aliases",
            MarkerValue::List(vec![
                string_value("SP"),
                string_value("systems perf"),
            ]),
        ),
        ("pages", MarkerValue::Number("42".to_string())),
    ]);

    let snapshot = super::projection_snapshot_json(&projection);
    assert_eq!(
        snapshot,
        r#"{"aliases":["SP","systems perf"],"pages":42,"parent":"[[obsidian]]","status":"wip","title":"Systems Performance"}"#
    );
    assert_eq!(
        super::projection_from_snapshot_json(&snapshot)
            .expect("parse snapshot"),
        projection
    );

    let canonicalized = super::projection_from_snapshot_json(
        r#"{"parent":"obsidian","status":"wip"}"#,
    )
    .expect("parse bare parent snapshot");
    assert_eq!(
        canonicalized.get("parent"),
        Some(&string_value("[[obsidian]]"))
    );
}

#[test]
fn deprecated_statuses_normalize_for_synced_inputs() {
    let marker = super::parse_marker_with_normalization(
        "- status: done\n- parent: obsidian\n",
    )
    .expect("parse deprecated marker status");
    assert!(marker.status_normalized.is_some());
    assert_eq!(marker.projection.get("status"), Some(&string_value("read")));

    let note = parse_note(
        "\
---
status: done
parent: obsidian
highlights_marker_base: '{\"parent\":\"obsidian\",\"status\":\"done\"}'
---

Body
",
    );
    let frontmatter = note
        .synced_projection_with_normalization()
        .expect("normalize frontmatter status");
    assert!(frontmatter.status_normalized.is_some());
    assert_eq!(
        frontmatter.projection.get("status"),
        Some(&string_value("read"))
    );

    let (base, base_status_normalized) = note
        .marker_base_projection_with_normalization()
        .expect("normalize base status");
    assert!(base_status_normalized.is_some());
    assert_eq!(
        base.expect("base projection").get("status"),
        Some(&string_value("read"))
    );

    let unread = super::parse_marker_with_normalization(
        "- status: unread\n- parent: obsidian\n",
    )
    .expect("parse deprecated unread marker status");
    assert!(unread.status_normalized.is_some());
    assert_eq!(
        unread.projection.get("status"),
        Some(&string_value("ready"))
    );
}

#[test]
fn projection_three_way_merge_handles_compatible_changes() {
    let base = test_projection(vec![
        ("status", string_value("wip")),
        ("parent", string_value("[[obsidian]]")),
        ("title", string_value("Old")),
    ]);

    let marker_only = super::merge_projection_changes(
        &base,
        &test_projection(vec![
            ("status", string_value("read")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]),
        &base,
    );
    assert!(marker_only.conflicts.is_empty());
    assert_eq!(
        marker_only.projection.get("status"),
        Some(&string_value("read"))
    );
    assert!(marker_only.marker_contributed);
    assert!(!marker_only.frontmatter_contributed);

    let frontmatter_only = super::merge_projection_changes(
        &base,
        &base,
        &test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("New")),
        ]),
    );
    assert!(frontmatter_only.conflicts.is_empty());
    assert_eq!(
        frontmatter_only.projection.get("title"),
        Some(&string_value("New"))
    );
    assert!(!frontmatter_only.marker_contributed);
    assert!(frontmatter_only.frontmatter_contributed);

    let same_value = super::merge_projection_changes(
        &base,
        &test_projection(vec![
            ("status", string_value("read")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]),
        &test_projection(vec![
            ("status", string_value("read")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]),
    );
    assert!(same_value.conflicts.is_empty());
    assert_eq!(
        same_value.projection.get("status"),
        Some(&string_value("read"))
    );
    assert!(same_value.marker_contributed);
    assert!(same_value.frontmatter_contributed);

    let non_overlapping = super::merge_projection_changes(
        &base,
        &test_projection(vec![
            ("status", string_value("read")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]),
        &test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("New")),
        ]),
    );
    assert!(non_overlapping.conflicts.is_empty());
    assert_eq!(
        non_overlapping.projection,
        test_projection(vec![
            ("status", string_value("read")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("New")),
        ])
    );
}

#[test]
fn projection_three_way_merge_handles_deletes_and_conflicts() {
    let base = test_projection(vec![
        ("status", string_value("wip")),
        ("parent", string_value("[[obsidian]]")),
        ("title", string_value("Old")),
    ]);
    let without_title = test_projection(vec![
        ("status", string_value("wip")),
        ("parent", string_value("[[obsidian]]")),
    ]);

    let delete_vs_unchanged =
        super::merge_projection_changes(&base, &without_title, &base);
    assert!(delete_vs_unchanged.conflicts.is_empty());
    assert!(!delete_vs_unchanged.projection.contains_key("title"));

    let delete_vs_change = super::merge_projection_changes(
        &base,
        &without_title,
        &test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("New")),
        ]),
    );
    assert_eq!(delete_vs_change.conflicts.len(), 1);
    assert_eq!(delete_vs_change.conflicts[0].key, "title");

    let same_key_different_value = super::merge_projection_changes(
        &base,
        &test_projection(vec![
            ("status", string_value("read")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]),
        &test_projection(vec![
            ("status", string_value("abandoned")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]),
    );
    assert_eq!(same_key_different_value.conflicts.len(), 1);
    assert_eq!(same_key_different_value.conflicts[0].key, "status");
}

#[test]
fn pdf_path_metadata_derives_nested_reference_paths() {
    let config = Config {
        bob_dir: PathBuf::from("/tmp/bob"),
        lib_dir: PathBuf::from("/tmp/bob/lib"),
        ref_dir: PathBuf::from("/tmp/bob/ref"),
        xlib_dir: PathBuf::from("/tmp/bob/xlib"),
    };

    let top_level =
        pdf_path_metadata(&config, Path::new("/tmp/bob/lib/example.pdf"))
            .expect("top-level metadata");
    assert_eq!(
        top_level.relative_pdf_path,
        Some(PathBuf::from("example.pdf"))
    );
    assert_eq!(top_level.note_relative_path, PathBuf::from("example.md"));
    assert_eq!(top_level.ref_type, None);
    assert_eq!(
        ref_note_path(&config, Path::new("/tmp/bob/lib/example.pdf"))
            .expect("top-level note path"),
        PathBuf::from("/tmp/bob/ref/example.md")
    );

    let nested =
        pdf_path_metadata(&config, Path::new("/tmp/bob/lib/books/example.pdf"))
            .expect("nested metadata");
    assert_eq!(
        nested.relative_pdf_path,
        Some(PathBuf::from("books/example.pdf"))
    );
    assert_eq!(nested.note_relative_path, PathBuf::from("books/example.md"));
    assert_eq!(nested.ref_type.as_deref(), Some("books"));
    assert_eq!(
        ref_note_path(&config, Path::new("/tmp/bob/lib/books/example.pdf"))
            .expect("nested note path"),
        PathBuf::from("/tmp/bob/ref/books/example.md")
    );

    let xlib_nested = pdf_path_metadata(
        &config,
        Path::new("/tmp/bob/xlib/books/example.pdf"),
    )
    .expect("xlib nested metadata");
    assert_eq!(xlib_nested.relative_pdf_path, nested.relative_pdf_path);
    assert_eq!(xlib_nested.note_relative_path, nested.note_relative_path);
    assert_eq!(xlib_nested.ref_type, nested.ref_type);

    let deeper = pdf_path_metadata(
        &config,
        Path::new("/tmp/bob/lib/books/os/example.PDF"),
    )
    .expect("deeper metadata");
    assert_eq!(
        deeper.note_relative_path,
        PathBuf::from("books/os/example.md")
    );
    assert_eq!(deeper.ref_type.as_deref(), Some("books"));

    let outside =
        pdf_path_metadata(&config, Path::new("/tmp/elsewhere/example.pdf"))
            .expect("outside metadata");
    assert_eq!(outside.relative_pdf_path, None);
    assert_eq!(outside.note_relative_path, PathBuf::from("example.md"));
    assert_eq!(outside.ref_type, None);
    assert_eq!(
        ref_note_path(&config, Path::new("/tmp/elsewhere/example.pdf"))
            .expect("outside note path"),
        PathBuf::from("/tmp/bob/ref/example.md")
    );
}

#[test]
fn stale_editor_save_requires_replaced_hash_base_and_mismatch() {
    assert!(is_stale_editor_save(
        Some("replaced"),
        Some("last"),
        true,
        "replaced",
    ));
    assert!(!is_stale_editor_save(
        Some("replaced"),
        Some("last"),
        true,
        "last",
    ));
    assert!(!is_stale_editor_save(
        Some("replaced"),
        Some("last"),
        true,
        "other",
    ));
    assert!(!is_stale_editor_save(
        Some("replaced"),
        Some("last"),
        false,
        "replaced",
    ));
    assert!(!is_stale_editor_save(None, Some("last"), true, "replaced"));
    assert!(!is_stale_editor_save(
        Some("replaced"),
        None,
        true,
        "replaced",
    ));
}

#[test]
fn replaced_hash_stores_on_marker_write_drops_on_agreement() {
    assert_eq!(
        replaced_hash_to_store(Some("old"), "pre-write", Some("last"), true)
            .as_deref(),
        Some("pre-write")
    );
    assert_eq!(
        replaced_hash_to_store(Some("old"), "last", Some("last"), false),
        None
    );
    assert_eq!(
        replaced_hash_to_store(Some("old"), "other", Some("last"), false)
            .as_deref(),
        Some("old")
    );
    assert!(is_dropping_replaced_hash(
        Some("last"),
        "last",
        false,
        Some("old")
    ));
    assert!(!is_dropping_replaced_hash(
        Some("last"),
        "last",
        true,
        Some("old")
    ));
}

#[test]
fn skip_stale_restore_without_write_opt_in_is_restore_alone() {
    let scan = SyncOptions {
        dry_run: false,
        write_pdf: false,
        prefer: None,
    };
    assert!(skip_stale_restore_without_write_opt_in(
        true, false, false, scan
    ));
    assert!(!skip_stale_restore_without_write_opt_in(
        true, true, false, scan
    ));
    assert!(!skip_stale_restore_without_write_opt_in(
        true, false, true, scan
    ));
    assert!(!skip_stale_restore_without_write_opt_in(
        true,
        false,
        false,
        SyncOptions {
            write_pdf: true,
            ..scan
        }
    ));
    assert!(!skip_stale_restore_without_write_opt_in(
        true,
        false,
        false,
        SyncOptions {
            dry_run: true,
            ..scan
        }
    ));
}

#[test]
fn finalize_replaced_hash_render_skips_a_drop_only_note_write() {
    let bob = temp_bob_dir("replaced-hash-drop-only");
    let path = bob.join("ref/example.md");
    write_test_file(
        &path,
        "---\nstatus: next\nhighlights_marker_replaced_hash: abcdef\n---\n\n# Example\n",
    );
    let note = read_note(&path).expect("read note");
    let rendered = "---\nstatus: next\n---\n\n# Example\n".to_string();
    let (out, kept) =
        finalize_replaced_hash_render(&note, rendered, None, true);
    assert_eq!(out, note.contents().expect("note contents"));
    assert_eq!(kept.as_deref(), Some("abcdef"));
    fs::remove_dir_all(bob).expect("remove temp bob dir");
}

#[test]
fn render_places_replaced_hash_after_marker_base() {
    let note = ParsedNote::empty();
    let mut projection = Projection::new();
    projection.insert(
        "status".to_string(),
        MarkerValue::String("next".to_string()),
    );
    projection.insert(
        "parent".to_string(),
        MarkerValue::String("[[obsidian]]".to_string()),
    );
    let hash = projection_hash(&projection).expect("hash");
    let rendered = note.render_with_projection(
        &projection,
        &hash,
        Some("deadbeef"),
        &PipelineMetadata {
            source_pdf: "lib/example.pdf".to_string(),
            source_pdf_sha256: "abc".to_string(),
            ref_type: None,
            highlights_sidecar: None,
            highlights_count: None,
            highlights_synced_at: None,
            created: None,
            audio: None,
            birth_lineage: None,
        },
        "",
    );
    let hash_at = rendered.find("highlights_marker_hash:").expect("hash line");
    let base_at = rendered.find("highlights_marker_base:").expect("base line");
    let replaced_at = rendered
        .find("highlights_marker_replaced_hash: deadbeef")
        .expect("replaced line");
    assert!(hash_at < base_at, "{rendered}");
    assert!(base_at < replaced_at, "{rendered}");
}

#[test]
fn render_keeps_block_earlier_list_byte_identical() {
    let note = parse_note(
        "---\nstatus: read\ntitle: Attention\nearlier:\n  - \"[[ref/papers/attention__v2|v2 · 2026-09-01]]\"\n  - \"[[ref/papers/attention|v1 · 2026-03-02]]\"\n---\n\n# Attention\n",
    );
    let mut projection = Projection::new();
    projection.insert(
        "status".to_string(),
        MarkerValue::String("read".to_string()),
    );
    projection.insert(
        "title".to_string(),
        MarkerValue::String("Attention".to_string()),
    );
    let hash = projection_hash(&projection).expect("hash");
    let rendered = note.render_with_projection(
        &projection,
        &hash,
        None,
        &PipelineMetadata {
            source_pdf: "lib/papers/attention.pdf".to_string(),
            source_pdf_sha256: "abc".to_string(),
            ref_type: None,
            highlights_sidecar: None,
            highlights_count: None,
            highlights_synced_at: None,
            created: None,
            audio: None,
            birth_lineage: None,
        },
        "# Attention\n",
    );
    let expected = "earlier:\n  - \"[[ref/papers/attention__v2|v2 · 2026-09-01]]\"\n  - \"[[ref/papers/attention|v1 · 2026-03-02]]\"\n";
    assert!(
        rendered.contains(expected),
        "block earlier: must survive sync byte-for-byte:\n{rendered}"
    );
}

#[test]
fn render_drops_continuations_of_a_removed_block_key() {
    // A stale block-style managed key (`parent:` with list items) is
    // re-rendered inline from the projection; its orphaned continuation
    // lines must not survive to be absorbed into the key above.
    let note = parse_note(
        "---\nstatus: read\nparent:\n  - obsidian\n  - extra\nearlier:\n  - \"[[ref/papers/attention|v1 · 2026-03-02]]\"\n---\n\n# Attention\n",
    );
    let mut projection = Projection::new();
    projection.insert(
        "status".to_string(),
        MarkerValue::String("read".to_string()),
    );
    projection.insert(
        "parent".to_string(),
        MarkerValue::String("[[obsidian]]".to_string()),
    );
    let hash = projection_hash(&projection).expect("hash");
    let rendered = note.render_with_projection(
        &projection,
        &hash,
        None,
        &PipelineMetadata {
            source_pdf: "lib/papers/attention.pdf".to_string(),
            source_pdf_sha256: "abc".to_string(),
            ref_type: None,
            highlights_sidecar: None,
            highlights_count: None,
            highlights_synced_at: None,
            created: None,
            audio: None,
            birth_lineage: None,
        },
        "# Attention\n",
    );
    assert!(
        !rendered.contains("  - obsidian") && !rendered.contains("  - extra"),
        "orphaned continuations of a removed key must go:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "earlier:\n  - \"[[ref/papers/attention|v1 · 2026-03-02]]\"\n"
        ),
        "earlier: still survives:\n{rendered}"
    );
}

#[test]
fn append_stale_save_reason_joins_existing_reason() {
    let mut empty = String::new();
    append_stale_save_reason(&mut empty);
    assert_eq!(empty, STALE_SAVE_REASON);
    let mut existing = String::from("frontmatter changed since last sync");
    append_stale_save_reason(&mut existing);
    assert_eq!(
        existing,
        format!("frontmatter changed since last sync; {STALE_SAVE_REASON}")
    );
}
