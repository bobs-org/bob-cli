//! Marker parse/render and frontmatter projection tests.
use super::*;

#[test]
fn marker_parser_accepts_yaml_subset_and_normalizes_keys() {
    let projection = parse_marker(
        "\
- Status: wip
* aliases: [\"Systems Performance\", linux]
- source-url: https://example.com/book
- parent: obsidian
- rating: 5
- archived: false
",
    )
    .expect("parse marker");

    assert_eq!(
        projection.get("status"),
        Some(&MarkerValue::String("wip".to_string()))
    );
    assert_eq!(
        projection.get("source_url"),
        Some(&MarkerValue::String("https://example.com/book".to_string()))
    );
    assert_eq!(
        projection.get("parent"),
        Some(&MarkerValue::String("[[obsidian]]".to_string()))
    );
    assert_eq!(
        projection.get("rating"),
        Some(&MarkerValue::Number("5".to_string()))
    );
    assert_eq!(projection.get("archived"), Some(&MarkerValue::Bool(false)));
}

#[test]
fn marker_parser_canonicalizes_parent_targets() {
    let bare = parse_marker("- status: wip\n- parent: obsidian\n")
        .expect("parse bare parent marker");
    assert_eq!(
        bare.get("parent"),
        Some(&MarkerValue::String("[[obsidian]]".to_string()))
    );

    let nested = parse_marker("- status: wip\n- parent: projects/foo\n")
        .expect("parse nested parent marker");
    assert_eq!(
        nested.get("parent"),
        Some(&MarkerValue::String("[[projects/foo]]".to_string()))
    );

    let spaced = parse_marker("- status: wip\n- parent: Systems Performance\n")
        .expect("parse spaced parent marker");
    assert_eq!(
        spaced.get("parent"),
        Some(&MarkerValue::String("[[Systems Performance]]".to_string()))
    );
}

#[test]
fn marker_parser_rejects_linked_parent_targets() {
    let cases = [
        (
            "- status: wip\n- parent: [[obsidian]]\n",
            "wikilinks are not supported",
        ),
        (
            "- status: wip\n- parent: [[obsidian|Obsidian]]\n",
            "aliases are not supported",
        ),
        (
            "- status: wip\n- parent: ![[obsidian]]\n",
            "embeds are not supported",
        ),
        (
            "- status: wip\n- parent: [[obsidian#^block]]\n",
            "block links are not supported",
        ),
        (
            "- status: wip\n- parent: \"obsidian\"\n",
            "quoted parent values are not supported",
        ),
    ];

    for (marker, expected_error) in cases {
        let error =
            parse_marker(marker).expect_err("linked parent should fail");
        assert!(
            error.to_string().contains(expected_error),
            "expected {expected_error:?} in {error}"
        );
    }
}

#[test]
fn frontmatter_projection_canonicalizes_parent_targets() {
    let note = parse_note(
        "\
---
status: wip
parent: obsidian
---

Body
",
    );
    let projection = note
        .synced_projection_with_normalization()
        .expect("extract frontmatter projection")
        .projection;
    assert_eq!(
        projection.get("parent"),
        Some(&MarkerValue::String("[[obsidian]]".to_string()))
    );

    let marker_projection = parse_marker("- status: wip\n- parent: obsidian\n")
        .expect("parse marker");
    let marker_hash = projection_hash(&marker_projection).expect("hash marker");
    let frontmatter_hash =
        projection_hash(&projection).expect("hash frontmatter");
    assert_eq!(marker_hash, frontmatter_hash);
}

#[test]
fn parent_canonicalization_rejects_non_scalar_values() {
    let marker_error = parse_marker("- status: wip\n- parent: [obsidian]\n")
        .expect_err("list parent marker should fail");
    assert!(
        marker_error
            .to_string()
            .contains("inline lists are not supported"),
        "{marker_error}"
    );

    let note = parse_note(
        "\
---
status: wip
parent: [obsidian]
---

Body
",
    );
    let frontmatter_error = note
        .synced_projection_with_normalization()
        .expect_err("list parent frontmatter should fail");
    assert!(
        frontmatter_error
            .to_string()
            .contains("frontmatter parent must be a scalar note target"),
        "{frontmatter_error}"
    );
}

#[test]
fn marker_parser_rejects_missing_required_keys_type_and_duplicate_status() {
    let missing = parse_marker("- title: Missing\n")
        .expect_err("missing status should fail");
    assert!(missing
        .to_string()
        .contains("missing required marker key: status"));

    let missing_parent = parse_marker("- status: wip\n")
        .expect_err("missing parent should fail");
    assert!(missing_parent
        .to_string()
        .contains("missing required marker key: parent"));

    let marker_type =
        parse_marker("- status: wip\n- parent: obsidian\n- type: [[book]]\n")
            .expect_err("marker type should fail");
    assert!(marker_type.to_string().contains("command-managed"));

    let marker_ref_type =
        parse_marker("- status: wip\n- parent: obsidian\n- ref_type: books\n")
            .expect_err("marker ref_type should fail");
    assert!(marker_ref_type.to_string().contains("command-managed"));

    let duplicate =
        parse_marker("- status: wip\n- parent: obsidian\n- Status: done\n")
            .expect_err("duplicate status should fail");
    assert!(duplicate.to_string().contains("duplicate marker key"));
}

#[test]
fn status_validation_rejects_unsupported_and_non_scalar_values() {
    let canonical = parse_marker("- status: read\n- parent: obsidian\n")
        .expect("read status should be supported");
    assert_eq!(
        canonical.get("status"),
        Some(&MarkerValue::String("read".to_string()))
    );

    let unsupported = parse_marker("- status: queued\n- parent: obsidian\n")
        .expect_err("unsupported status should fail");
    assert!(
        unsupported
            .to_string()
            .contains("marker has unsupported status \"queued\""),
        "{unsupported}"
    );

    let non_scalar = parse_marker("- status: [wip]\n- parent: obsidian\n")
        .expect_err("list status should fail");
    assert!(
        non_scalar
            .to_string()
            .contains("marker status must be a scalar string"),
        "{non_scalar}"
    );
}

#[test]
fn marker_renderer_uses_stable_key_order() {
    let mut projection = Projection::new();
    projection.insert(
        "z_custom".to_string(),
        MarkerValue::String("last".to_string()),
    );
    projection
        .insert("status".to_string(), MarkerValue::String("wip".to_string()));
    projection.insert(
        "title".to_string(),
        MarkerValue::String("Systems Performance".to_string()),
    );
    projection.insert(
        "parent".to_string(),
        MarkerValue::String("[[obsidian]]".to_string()),
    );

    assert_eq!(
        render_marker(&projection).expect("render marker"),
        "\
- status: wip
- parent: obsidian
- title: Systems Performance
- z_custom: last
"
    );
}

#[test]
fn marker_renderer_rejects_unrepresentable_parent_links() {
    let mut projection = Projection::new();
    projection
        .insert("status".to_string(), MarkerValue::String("wip".to_string()));
    projection.insert(
        "parent".to_string(),
        MarkerValue::String("[[obsidian|Obsidian]]".to_string()),
    );

    let error =
        render_marker(&projection).expect_err("alias parent should fail");
    assert!(
        error.to_string().contains(
            "parent cannot be rendered as a PDF marker bare note target"
        ),
        "{error}"
    );
}

#[test]
fn frontmatter_projection_uses_marker_fields_without_fallback_parent() {
    let note = parse_note(
        "\
---
status: wip
title: Existing
type: \"[[old-type]]\"
custom_flag: true
highlights_marker_fields: [custom_flag]
source_pdf: lib/example.pdf
---

Body
",
    );

    let projection = note
        .synced_projection_with_normalization()
        .expect("extract frontmatter projection")
        .projection;
    assert!(!projection.contains_key("parent"));
    assert!(!projection.contains_key("type"));
    assert_eq!(
        projection.get("custom_flag"),
        Some(&MarkerValue::Bool(true))
    );
    assert!(!projection.contains_key("source_pdf"));
}

#[test]
fn frontmatter_render_preserves_unmanaged_keys() {
    let note = parse_note(
        "\
---
status: legacy
type: \"[[old-type]]\"
owner: Bryan
---

Manual body.
",
    );
    let mut projection = Projection::new();
    projection
        .insert("status".to_string(), MarkerValue::String("wip".to_string()));
    projection.insert(
        "parent".to_string(),
        MarkerValue::String("[[obsidian]]".to_string()),
    );
    let hash = projection_hash(&projection).expect("hash projection");
    let rendered = note.render_with_projection(
        &projection,
        &hash,
        &PipelineMetadata {
            source_pdf: "lib/example.pdf".to_string(),
            source_pdf_sha256: "abc123".to_string(),
            ref_type: None,
            highlights_sidecar: None,
            highlights_count: None,
            highlights_synced_at: None,
        },
        &note.body,
    );

    assert!(rendered.contains("status: wip\n"));
    assert!(rendered.contains("type: \"[[ref]]\"\n"));
    assert!(!rendered.contains("type: \"[[old-type]]\"\n"));
    assert!(rendered.contains("owner: Bryan\n"));
    assert!(rendered.contains("source_pdf: lib/example.pdf\n"));
    assert!(rendered.ends_with("\nManual body.\n"));
}
