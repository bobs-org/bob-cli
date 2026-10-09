//! Focused tests for the read-only ref index over the mixed-corpus
//! fixture vault: one assertion per row field and per diagnostic code, query
//! classification and scoring tables, supersession, determinism, and a
//! side-effect check.
use std::collections::BTreeSet;
use std::path::PathBuf;

use super::resolve::title_score;
use super::*;

fn vault_dir() -> PathBuf {
    PathBuf::from("tests/fixtures/ref_library/vault")
}

fn fixture_config() -> LibraryConfig {
    let vault = vault_dir();
    LibraryConfig::for_ref_dir(vault.clone(), vault.join("ref"))
}

fn fixture_index() -> RefIndex {
    build_index(&fixture_config()).expect("build fixture index")
}

fn row<'a>(index: &'a RefIndex, path: &str) -> &'a RefRow {
    index
        .row_by_path(path)
        .unwrap_or_else(|| panic!("missing fixture row {path}"))
}

fn codes(row: &RefRow) -> BTreeSet<&str> {
    row.diagnostics
        .iter()
        .map(|item| item.code.as_str())
        .collect()
}

#[test]
fn index_membership_and_coverage() {
    let index = fixture_index();
    assert_eq!(index.rows.len(), 34);
    assert_eq!(index.coverage.notes, 34);
    assert_eq!(index.coverage.skipped, 3);
    assert_eq!(index.coverage.ref_dir, "ref");
    assert_eq!(index.coverage.intake, "not_checked");
    assert!(!index.coverage.scope.is_empty());
    assert!(!index.coverage.annotations.is_empty());
    // The root hub, the conflict copy, hidden and asset notes, and the
    // non-markdown file are all outside the membership.
    for path in [
        "hub_ref.md",
        "ref/papers/real_note (conflict copy).md",
        "ref/.hidden/hidden_note.md",
        "ref/papers/note.assets/asset.md",
        "ref/papers/ignore.txt",
    ] {
        assert!(index.row_by_path(path).is_none(), "indexed {path}");
    }
    let paths = index
        .rows
        .iter()
        .map(|row| row.path.clone())
        .collect::<Vec<_>>();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "rows are built in path order");
}

#[test]
fn synced_paper_row_covers_the_core_fields() {
    let index = fixture_index();
    let row = row(&index, "ref/papers/synced_paper.md");
    assert_eq!(row.link, "[[ref/papers/synced_paper]]");
    assert_eq!(row.title, "Synced Paper on Reference Indexing");
    assert_eq!(row.origin, "external");
    assert_eq!(row.ref_type.as_deref(), Some("papers"));
    assert_eq!(row.era, "modern");
    assert_eq!(row.status.as_deref(), Some("ready"));
    assert_eq!(row.status_sync, "ok");
    assert_eq!(row.frontmatter_status, None);
    assert_eq!(row.legacy_status, None);
    assert_eq!(row.reading_state, "queued");
    assert_eq!(row.reading_state_source, "ref_task:[ ]");
    assert_eq!(row.parent.as_deref(), Some("reading_list"));
    assert_eq!(
        row.urls,
        vec![
            "https://example.com/papers/synced-paper".to_string(),
            "https://example.com/papers/synced-paper-v2".to_string(),
        ]
    );
    assert_eq!(row.identity.arxiv, None);
    assert_eq!(row.identity.doi, None);
    assert!(row
        .identity
        .keys
        .contains(&"https://example.com/papers/synced-paper".to_string()));
    assert_eq!(row.annotation_count, 1);
    assert_eq!(row.comment_count, 1);
    let snapshot = row.snapshot.as_ref().expect("synced snapshot");
    assert_eq!(snapshot.synced_at.as_deref(), Some("2026-09-20T10:00:00"));
    assert_eq!(
        snapshot.highlights_count,
        Some(serde_json::Value::Number(3.into()))
    );
    assert_eq!(row.research_ref, None);
    assert_eq!(row.superseded_by, None);
    assert_eq!(
        codes(row),
        BTreeSet::from(["marker_mirror_excluded", "open_v1_tracker"])
    );
    assert!(!row.blocked);
}

#[test]
fn clipped_blog_row_covers_media_dates_and_tombstones() {
    let index = fixture_index();
    let row = row(&index, "ref/blogs/clipped_blog.md");
    assert_eq!(row.status.as_deref(), Some("read"));
    assert_eq!(row.status_sync, "ok");
    assert_eq!(row.reading_state, "finished");
    assert_eq!(row.reading_state_source, "ref_task:[x]");
    // Frontmatter `ref_type` wins over the directory name.
    assert_eq!(row.ref_type.as_deref(), Some("blog"));
    assert_eq!(row.origin, "external");
    assert_eq!(row.author.as_deref(), Some("Ada Lovelace"));
    assert_eq!(row.published.as_deref(), Some("2026-08-01"));
    assert_eq!(row.captured.as_deref(), Some("2026-08-15"));
    assert_eq!(row.added.as_deref(), Some("2026-08-15"));
    assert_eq!(row.added_source.as_deref(), Some("captured"));
    assert_eq!(row.finished.as_deref(), Some("2026-08-18"));
    assert_eq!(row.finished_source.as_deref(), Some("ref_task"));
    assert_eq!(
        row.source_pdf.as_deref(),
        Some("lib/blogs/capture-flows.pdf")
    );
    assert_eq!(row.audio.as_deref(), Some("lib/blogs/capture-flows.mp3"));
    // Four live blocks (highlight, commented highlight, note, image); the
    // tombstone is recorded nowhere in the counts.
    assert_eq!(row.annotation_count, 4);
    assert_eq!(row.comment_count, 2);
    assert!(row.diagnostics.is_empty());
}

#[test]
fn legacy_unread_is_superseded_by_the_pdf_capture() {
    let index = fixture_index();
    let row = row(&index, "ref/ai/legacy_unread.md");
    assert_eq!(row.era, "legacy");
    assert_eq!(row.status.as_deref(), Some("legacy"));
    assert_eq!(row.legacy_status.as_deref(), Some("unread"));
    assert_eq!(row.reading_state, "queued");
    assert_eq!(row.reading_state_source, "legacy_status:unread");
    assert_eq!(
        row.superseded_by.as_deref(),
        Some("ref/blogs/clipped_blog.md")
    );
    assert_eq!(row.added.as_deref(), Some("2026-01-01"));
    assert_eq!(row.added_source.as_deref(), Some("zorg_block"));
    // Library counts exclude superseded rows.
    assert_eq!(index.counts.notes, 33);
}

#[test]
fn legacy_status_table_maps_every_reading_state() {
    let index = fixture_index();
    for (path, state, source) in [
        (
            "ref/ai/legacy_collect.md",
            "started",
            "legacy_status:collect_fleeting_notes",
        ),
        (
            "ref/ai/legacy_review_lit.md",
            "finished",
            "legacy_status:review_lit_notes",
        ),
        (
            "ref/ai/legacy_review_fleeting.md",
            "finished",
            "legacy_status:review_fleeting_notes",
        ),
        ("ref/ai/legacy_read.md", "finished", "legacy_status:read"),
        (
            "ref/ai/legacy_abandoned.md",
            "dropped",
            "legacy_status:abandoned",
        ),
        ("ref/ai/legacy_book.md", "unknown", "legacy_status:book"),
        (
            "ref/ai/legacy_missing.md",
            "unknown",
            "legacy_status:missing",
        ),
    ] {
        let row = row(&index, path);
        assert_eq!(row.era, "legacy", "{path}");
        assert_eq!(row.reading_state, state, "{path}");
        assert_eq!(row.reading_state_source, source, "{path}");
    }
}

#[test]
fn chat_notes_are_agent_reports_with_queue_states() {
    let index = fixture_index();
    for (path, state) in [
        ("ref/chat/chat_next.md", "queued"),
        ("ref/chat/chat_ready.md", "queued"),
        ("ref/chat/chat_read.md", "finished"),
        ("ref/chat/chat_abandoned.md", "dropped"),
    ] {
        let row = row(&index, path);
        assert_eq!(row.origin, "agent-report", "{path}");
        assert_eq!(row.ref_type.as_deref(), Some("chat"), "{path}");
        assert_eq!(row.reading_state, state, "{path}");
    }
    let read = row(&index, "ref/chat/chat_read.md");
    assert_eq!(
        read.research_ref.as_deref(),
        Some("research:202610/some_topic")
    );
    // A list-valued parent keeps its first entry.
    assert_eq!(read.parent.as_deref(), Some("inbox"));
    let next = row(&index, "ref/chat/chat_next.md");
    assert_eq!(next.reading_state_source, "ref_task:[*]");
}

#[test]
fn pending_and_conflict_status_precedence() {
    let index = fixture_index();
    let pending = row(&index, "ref/papers/pending_note.md");
    assert_eq!(pending.status.as_deref(), Some("read"));
    assert_eq!(pending.status_sync, "pending");
    assert_eq!(pending.frontmatter_status.as_deref(), Some("ready"));
    assert_eq!(pending.reading_state, "finished");
    // Finished without a tracker date stays dateless until the Git pass.
    assert_eq!(pending.finished, None);
    assert_eq!(pending.finished_source, None);

    let conflict = row(&index, "ref/papers/conflict_note.md");
    assert_eq!(conflict.status.as_deref(), Some("conflict"));
    assert_eq!(conflict.status_sync, "conflict");
    assert_eq!(conflict.frontmatter_status.as_deref(), Some("next"));
    assert_eq!(conflict.reading_state, "unknown");
    assert_eq!(
        conflict.reading_state_source,
        "conflict:ref_task=[x],frontmatter=next"
    );
    assert_eq!(codes(conflict), BTreeSet::from(["status_conflict"]));
}

#[test]
fn tracker_diagnostics_fall_back_to_frontmatter() {
    let index = fixture_index();
    let two = row(&index, "ref/papers/two_trackers.md");
    assert_eq!(
        codes(two),
        BTreeSet::from(["multiple_ref_trackers", "open_v1_tracker"])
    );
    assert_eq!(two.status.as_deref(), Some("wip"));
    assert_eq!(two.reading_state, "started");
    assert_eq!(two.reading_state_source, "frontmatter:wip");
    assert!(!two.blocked);

    // Several trackers with one `[?]` still leave `blocked` false: only
    // exactly one `^ref` tracker with a `[?]` mark sets the overlay.
    let pair = row(&index, "ref/papers/two_trackers_blocked.md");
    assert_eq!(
        codes(pair),
        BTreeSet::from(["multiple_ref_trackers", "open_v1_tracker"])
    );
    assert!(!pair.blocked);
    assert_eq!(pair.reading_state, "unknown");
    assert_eq!(pair.reading_state_source, "none");

    let unknown = row(&index, "ref/papers/unknown_mark.md");
    assert_eq!(
        codes(unknown),
        BTreeSet::from(["unknown_ref_mark", "open_v1_tracker"])
    );
    assert!(!unknown.blocked);
    assert_eq!(unknown.status.as_deref(), Some("ready"));
    assert_eq!(unknown.reading_state, "queued");
    // No frontmatter title: the first H1 is the title.
    assert_eq!(unknown.title, "Unknown Mark Note");
    assert_eq!(unknown.snapshot, None);
}

#[test]
fn blocked_tracker_defers_to_frontmatter_without_diagnostic() {
    let index = fixture_index();
    let blocked = row(&index, "ref/papers/blocked_mark.md");
    assert_eq!(blocked.status.as_deref(), Some("next"));
    assert_eq!(blocked.status_sync, "ok");
    assert_eq!(blocked.reading_state, "queued");
    assert_eq!(blocked.reading_state_source, "ref_task:[?]");
    assert_eq!(codes(blocked), BTreeSet::from(["open_v1_tracker"]));
    assert_eq!(blocked.title, "Blocked Mark Note");
    assert!(blocked.blocked);
}

#[test]
fn malformed_yaml_falls_back_and_reports() {
    let index = fixture_index();
    let row = row(&index, "ref/papers/bad_yaml.md");
    assert!(codes(row).contains("invalid_yaml"));
    assert_eq!(row.status.as_deref(), Some("ready"));
    assert_eq!(row.title, "[unclosed list");
    // The fallback keeps the unquoted wikilink intact, so the parent
    // reads as the bare note name.
    assert_eq!(row.parent.as_deref(), Some("fallback_parent"));
}

#[test]
fn wikilink_parent_and_opaque_url() {
    let index = fixture_index();
    let wiki = row(&index, "ref/papers/wiki_parent.md");
    assert_eq!(wiki.parent.as_deref(), Some("sase_ref"));

    let go = row(&index, "ref/blogs/go_link.md");
    assert_eq!(codes(go), BTreeSet::from(["opaque_url", "open_v1_tracker"]));
    assert_eq!(go.identity.keys, vec!["raw:go/capture-thing".to_string()]);
}

#[test]
fn arxiv_and_doi_identities_share_keys_and_conflict() {
    let index = fixture_index();
    let pdf = row(&index, "ref/papers/arxiv_pdf.md");
    assert!(pdf
        .identity
        .keys
        .contains(&"https://arxiv.org/abs/1706.03762".to_string()));
    assert!(pdf.identity.keys.contains(&"arxiv:1706.03762".to_string()));
    assert_eq!(pdf.identity.arxiv.as_deref(), Some("1706.03762"));
    assert_eq!(codes(pdf), BTreeSet::from(["duplicate_identity"]));

    let doi = row(&index, "ref/papers/arxiv_doi.md");
    assert!(doi
        .identity
        .keys
        .contains(&"doi:10.48550/arxiv.1706.03762".to_string()));
    assert!(doi.identity.keys.contains(&"arxiv:1706.03762".to_string()));
    assert_eq!(
        doi.identity.doi.as_deref(),
        Some("10.48550/arxiv.1706.03762")
    );
    assert_eq!(doi.identity.arxiv.as_deref(), Some("1706.03762"));
    assert_eq!(
        codes(doi),
        BTreeSet::from(["duplicate_identity", "open_v1_tracker"])
    );
    // Both notes are PDF-backed, so neither is superseded.
    assert_eq!(pdf.superseded_by, None);
    assert_eq!(doi.superseded_by, None);
}

#[test]
fn tracker_dates_cover_bracket_cancelled_and_emoji_forms() {
    let index = fixture_index();
    for (path, date) in [
        ("ref/papers/dates_done.md", "2026-09-01"),
        ("ref/papers/dates_cancelled.md", "2026-07-04"),
        ("ref/papers/dates_emoji.md", "2026-08-15"),
    ] {
        let row = row(&index, path);
        assert_eq!(row.finished.as_deref(), Some(date), "{path}");
        assert_eq!(row.finished_source.as_deref(), Some("ref_task"), "{path}");
    }
    let cancelled = row(&index, "ref/papers/dates_cancelled.md");
    assert_eq!(cancelled.reading_state, "dropped");
}

#[test]
fn added_dates_cover_created_and_zorg_block() {
    let index = fixture_index();
    let created = row(&index, "ref/papers/created_note.md");
    assert_eq!(created.added.as_deref(), Some("2026-09-10"));
    assert_eq!(created.added_source.as_deref(), Some("created"));
    // No title and no H1: the humanized stem is the title.
    assert_eq!(created.title, "created note");

    let zorg = row(&index, "ref/ai/zorg_block.md");
    assert_eq!(zorg.added.as_deref(), Some("2025-12-31"));
    assert_eq!(zorg.added_source.as_deref(), Some("zorg_block"));

    let unknown = row(&index, "ref/papers/unknown_mark.md");
    assert_eq!(unknown.added, None);
    assert_eq!(unknown.added_source, None);
}

#[test]
fn top_level_note_has_no_status_type_or_tracker() {
    let index = fixture_index();
    let row = row(&index, "ref/toplevel_note.md");
    assert_eq!(row.status, None);
    assert_eq!(row.reading_state, "unknown");
    assert_eq!(row.reading_state_source, "none");
    assert_eq!(row.ref_type, None);
    assert_eq!(codes(row), BTreeSet::from(["missing_type"]));
}

#[test]
fn region_oddities_exclude_preamble_and_report_unparsed() {
    let index = fixture_index();
    let row = row(&index, "ref/papers/real_note.md");
    assert_eq!(row.annotation_count, 1);
    assert_eq!(row.comment_count, 0);
    assert_eq!(
        codes(row),
        BTreeSet::from([
            "preamble_excluded",
            "unparsed_region",
            "open_v1_tracker"
        ])
    );
}

#[test]
fn every_diagnostic_code_is_represented() {
    let index = fixture_index();
    let mut all = BTreeSet::new();
    for row in &index.rows {
        for diagnostic in &row.diagnostics {
            all.insert(diagnostic.code.as_str());
        }
    }
    assert_eq!(
        all,
        BTreeSet::from([
            "invalid_yaml",
            "multiple_ref_trackers",
            "unknown_ref_mark",
            "status_conflict",
            "marker_mirror_excluded",
            "preamble_excluded",
            "unparsed_region",
            "opaque_url",
            "duplicate_identity",
            "missing_type",
            "open_v1_tracker",
        ])
    );
}

#[test]
fn query_classification_table() {
    for (query, kind) in [
        ("https://example.com/x", QueryKind::Url),
        ("HTTP://example.com/x", QueryKind::Url),
        ("1706.03762", QueryKind::Arxiv),
        ("arxiv:1706.03762", QueryKind::Arxiv),
        ("1706.03762v2", QueryKind::Arxiv),
        ("hep-th/9901001", QueryKind::Arxiv),
        ("math.GT/0309136v2", QueryKind::Arxiv),
        ("10.48550/arXiv.1706.03762", QueryKind::Doi),
        ("doi:10.1234/abc-def", QueryKind::Doi),
        ("ref/papers/x", QueryKind::Path),
        ("note.pdf", QueryKind::Path),
        ("ea_graph", QueryKind::Name),
        ("attention is all you need", QueryKind::Title),
    ] {
        assert_eq!(classify_query(query), kind, "{query}");
    }
}

#[test]
fn title_scoring_table() {
    // Normalized equality scores 100.
    assert_eq!(
        title_score("Attention Is All You Need", "attention is all you need"),
        100
    );
    // Stopwords drop out before the Dice score: 2*2/(2+4) rounds to 67.
    assert_eq!(
        title_score("capture flows", "Clipped Blog on Capture Flows"),
        67
    );
    // Containment with a 3-token shorter side rises to at least 90.
    assert_eq!(
        title_score(
            "capture flows roundup",
            "Capture Flows Roundup Extra Notes Here"
        ),
        90
    );
    // A 2-token containment stays at its Dice value.
    assert_eq!(title_score("capture flows", "Capture Flows Roundup"), 80);
    assert_eq!(title_score("the", "the"), 0);
    assert_eq!(title_score("", "Anything"), 0);
    assert_eq!(title_score("no shared words here", "entirely different"), 0);
}

#[test]
fn url_queries_match_identity_including_www_variants() {
    let index = fixture_index();
    for query in [
        "https://example.com/blog/capture-flows",
        "https://www.example.com/blog/capture-flows/",
    ] {
        let hits = resolve_query(&index.rows, &index.bob_dir, query);
        // The PDF capture and its superseded legacy twin share the key;
        // the non-superseded row orders first.
        assert_eq!(hits.len(), 2, "{query}");
        assert!(hits.iter().all(|hit| hit.kind == MatchKind::Identity));
        assert_eq!(
            index.rows[hits[0].row].path, "ref/blogs/clipped_blog.md",
            "{query}"
        );
        assert_eq!(
            index.rows[hits[1].row].path, "ref/ai/legacy_unread.md",
            "{query}"
        );
    }
}

#[test]
fn arxiv_queries_strip_versions_and_order_by_state() {
    let index = fixture_index();
    for query in [
        "https://arxiv.org/abs/1706.03762",
        "https://arxiv.org/pdf/1706.03762v2",
        "arxiv:1706.03762",
        "1706.03762v2",
    ] {
        let hits = resolve_query(&index.rows, &index.bob_dir, query);
        assert_eq!(hits.len(), 2, "{query}");
        assert!(hits.iter().all(|hit| hit.kind == MatchKind::Identity));
        // Finished outranks queued in primary-match order.
        assert_eq!(
            index.rows[hits[0].row].path, "ref/papers/arxiv_pdf.md",
            "{query}"
        );
    }
    // A bare arXiv DOI derives the stored-side `doi:` key plus the
    // `arxiv:` key, so it matches both twin notes in primary-match order.
    let doi =
        resolve_query(&index.rows, &index.bob_dir, "10.48550/arXiv.1706.03762");
    assert_eq!(doi.len(), 2);
    assert!(doi.iter().all(|hit| hit.kind == MatchKind::Identity));
    assert_eq!(index.rows[doi[0].row].path, "ref/papers/arxiv_pdf.md");
    assert_eq!(index.rows[doi[1].row].path, "ref/papers/arxiv_doi.md");
    // A percent-encoded DOI URL query matches the same twins.
    let encoded = resolve_query(
        &index.rows,
        &index.bob_dir,
        "https://doi.org/10.48550%2FarXiv.1706.03762",
    );
    assert_eq!(encoded.len(), 2);
    assert!(encoded.iter().all(|hit| hit.kind == MatchKind::Identity));
    assert_eq!(index.rows[encoded[0].row].path, "ref/papers/arxiv_pdf.md");
    assert_eq!(index.rows[encoded[1].row].path, "ref/papers/arxiv_doi.md");
}

#[test]
fn path_name_and_title_queries() {
    let index = fixture_index();
    let by_path =
        resolve_query(&index.rows, &index.bob_dir, "ref/papers/synced_paper");
    assert_eq!(by_path.len(), 1);
    assert_eq!(by_path[0].kind, MatchKind::Path);

    let by_pdf = resolve_query(
        &index.rows,
        &index.bob_dir,
        "lib/papers/synced_paper.pdf",
    );
    assert_eq!(by_pdf.len(), 1);
    assert_eq!(by_pdf[0].kind, MatchKind::SourcePdf);

    let by_stem = resolve_query(&index.rows, &index.bob_dir, "dup_stem");
    assert_eq!(by_stem.len(), 2);
    assert!(by_stem.iter().all(|hit| hit.kind == MatchKind::Stem));

    let by_title =
        resolve_query(&index.rows, &index.bob_dir, "Attention Is All You Need");
    assert_eq!(by_title[0].kind, MatchKind::TitleExact);
    assert_eq!(index.rows[by_title[0].row].path, "ref/papers/arxiv_pdf.md");

    // More than five title matches collapse to the top five by score.
    let many = resolve_query(&index.rows, &index.bob_dir, "legacy");
    assert_eq!(many.len(), 5);
    let scores = many
        .iter()
        .map(|hit| hit.score.unwrap_or(0))
        .collect::<Vec<_>>();
    let mut ordered = scores.clone();
    ordered.sort_by(|a, b| b.cmp(a));
    assert_eq!(scores, ordered);
}

#[test]
fn url_miss_falls_back_to_slug_title_candidates() {
    let index = fixture_index();
    let hits = resolve_query(
        &index.rows,
        &index.bob_dir,
        "https://example.com/blog/capture-flows-roundup",
    );
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|hit| hit.kind == MatchKind::SlugTitle));
    assert_eq!(index.rows[hits[0].row].path, "ref/blogs/clipped_blog.md");
}

#[test]
fn same_index_twice_serializes_identically() {
    let first = fixture_index();
    let second = fixture_index();
    let to_json = |index: &RefIndex| {
        serde_json::to_string(&index.rows).expect("serialize rows")
    };
    assert_eq!(to_json(&first), to_json(&second));
}

#[test]
fn building_the_index_writes_nothing() {
    let before = fixture_bytes();
    let _ = fixture_index();
    assert_eq!(fixture_bytes(), before);
}

#[test]
fn missing_ref_dir_is_an_error() {
    let vault = vault_dir();
    let config = LibraryConfig::for_ref_dir(
        vault.clone(),
        vault.join("ref-does-not-exist"),
    );
    let error = build_index(&config).expect_err("missing ref dir");
    assert!(error.to_string().contains("reference directory not found"));
}

fn fixture_bytes() -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault_dir().join("ref")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read fixture dir") {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                entries.push((
                    path.clone(),
                    std::fs::read(&path).expect("read fixture file"),
                ));
            }
        }
    }
    entries.sort();
    entries
}
