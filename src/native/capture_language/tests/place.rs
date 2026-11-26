//! Cross-scope place lexer and resolver: one ladder, three adapters.

use super::super::place::{
    format_notebook_section_create_title, quoted_after_hash, scan_hash_opener,
    NotebookSection, PlaceCandidate, PlaceCatalog, PlaceOutcome,
    ProjectSection, TaskDrawer,
};
use super::*;
use crate::native::capture_drawers::is_reserved_drawer_key;
use std::collections::HashMap;

struct EmptyCatalog;

impl PlaceCatalog for EmptyCatalog {
    fn spelling_for_slug(&self, slug: &str) -> String {
        format_notebook_section_create_title(slug)
    }

    fn near_title(&self, _: &str) -> Option<String> {
        None
    }
}

struct MarkerCatalog {
    explicit: HashMap<String, String>,
}

impl PlaceCatalog for MarkerCatalog {
    fn spelling_for_slug(&self, slug: &str) -> String {
        let key = super::super::place::place_key(&slug.replace('-', " "));
        self.explicit
            .get(&key)
            .cloned()
            .unwrap_or_else(|| format_notebook_section_create_title(slug))
    }

    fn near_title(&self, _: &str) -> Option<String> {
        None
    }
}

fn drawer(title: &str, line: usize) -> PlaceCandidate {
    PlaceCandidate::new(
        title,
        crate::native::capture_drawers::drawer_key(title),
        selector_slug(title),
        line,
    )
}

fn heading(title: &str, line_index: usize) -> PlaceCandidate {
    PlaceCandidate::from_heading(title, line_index)
}

fn existing_title(outcome: PlaceOutcome) -> String {
    match outcome {
        PlaceOutcome::Existing(item) => item.title,
        other => panic!("expected existing: {other:?}"),
    }
}

fn create_title(outcome: PlaceOutcome) -> String {
    match outcome {
        PlaceOutcome::Create { title, .. } => title,
        other => panic!("expected create: {other:?}"),
    }
}

#[test]
fn leading_hash_opener_is_shared() {
    let hash = scan_hash_opener("#future-work").expect("slug");
    assert_eq!(hash.start, 0);
    assert_eq!(hash.sigil, 0);
    assert!(!hash.escaped);

    let quoted = scan_hash_opener("  #\"API Design\"").expect("quoted");
    assert_eq!(quoted.start, 2);
    assert_eq!(quoted.sigil, 2);

    let escaped = scan_hash_opener("\\#idea").expect("escaped");
    assert!(escaped.escaped);
    assert_eq!(escaped.sigil, 1);

    assert!(scan_hash_opener("#1").is_none());
    assert!(scan_hash_opener("##heading").is_none());
    assert!(scan_hash_opener("#-dash").is_none());
    assert!(scan_hash_opener("#").is_some());
}

#[test]
fn quoted_after_hash_keeps_escapes_smart_quotes_and_incomplete() {
    let complete = quoted_after_hash("\"NON-GOALS\"").expect("quoted");
    assert_eq!(complete.decoded, "NON-GOALS");
    assert!(complete.complete);
    assert_eq!(complete.literal_len, "\"NON-GOALS\"".len());

    let smart = quoted_after_hash("“café”").expect("smart");
    assert_eq!(smart.decoded, "café");
    assert!(smart.complete);

    let escaped = quoted_after_hash("\"a\\\"b\"").expect("escape");
    assert_eq!(escaped.decoded, "a\"b");

    let incomplete = quoted_after_hash("\"API").expect("open");
    assert!(!incomplete.complete);
    assert_eq!(incomplete.decoded, "API");

    assert!(quoted_after_hash("future-work").is_none());
}

#[test]
fn quoted_exact_then_key_then_slug_then_create() {
    let drawers = vec![drawer("FUTURE WORK", 3), drawer("REQUIREMENTS", 7)];
    let headings = vec![heading("Git", 8), heading("Merge Tools", 20)];

    assert_eq!(
        existing_title(TaskDrawer::resolve(
            "Future Work",
            true,
            &drawers,
            &EmptyCatalog,
            is_reserved_drawer_key,
        )),
        "FUTURE WORK"
    );
    assert_eq!(
        existing_title(ProjectSection::resolve(
            "future-work",
            false,
            &drawers,
            &EmptyCatalog,
            is_reserved_drawer_key,
        )),
        "FUTURE WORK"
    );
    assert_eq!(
        existing_title(NotebookSection::resolve("Git", true, &headings)),
        "Git"
    );
    assert_eq!(
        existing_title(NotebookSection::resolve(
            "merge-tools",
            false,
            &headings,
        )),
        "Merge Tools"
    );

    assert_eq!(
        create_title(TaskDrawer::resolve(
            "api-design",
            false,
            &drawers,
            &EmptyCatalog,
            is_reserved_drawer_key,
        )),
        "Api Design"
    );
    assert_eq!(
        create_title(ProjectSection::resolve(
            "api-design",
            false,
            &drawers,
            &EmptyCatalog,
            is_reserved_drawer_key,
        )),
        "Api Design"
    );
    assert_eq!(
        create_title(TaskDrawer::resolve(
            "API Design",
            true,
            &drawers,
            &EmptyCatalog,
            is_reserved_drawer_key,
        )),
        "API Design"
    );
    assert_eq!(
        create_title(NotebookSection::resolve("API-keys", false, &headings)),
        "API Keys"
    );
    assert_eq!(
        create_title(NotebookSection::resolve("API Design", true, &headings)),
        "API Design"
    );
}

#[test]
fn case_twins_and_duplicate_titles_are_ambiguous_with_lines() {
    let twins = vec![drawer("NOTES", 4), drawer("Notes", 9)];
    match TaskDrawer::resolve(
        "notes",
        false,
        &twins,
        &EmptyCatalog,
        is_reserved_drawer_key,
    ) {
        PlaceOutcome::Refuse { code, message } => {
            assert_eq!(code, "ambiguous_drawer");
            assert!(message.contains("lines 4, 9"), "{message}");
        }
        other => panic!("case twins refuse: {other:?}"),
    }

    let duplicates = vec![heading("Git", 8), heading("Git", 22)];
    match NotebookSection::resolve("git", false, &duplicates) {
        PlaceOutcome::Ambiguous { matches } => {
            assert_eq!(matches.len(), 2);
            assert_eq!(matches[0].line, 8);
            assert_eq!(matches[1].line, 22);
        }
        other => panic!("duplicate titles are ambiguous: {other:?}"),
    }
}

#[test]
fn reserved_and_structural_names_keep_scope_nouns() {
    match TaskDrawer::resolve(
        "tasks",
        false,
        &[],
        &EmptyCatalog,
        is_reserved_drawer_key,
    ) {
        PlaceOutcome::Refuse { code, message } => {
            assert_eq!(code, "reserved_drawer");
            assert_eq!(
                message,
                "`TASKS` is managed by Bob; name the drawer something else"
            );
        }
        other => panic!("reserved task drawer: {other:?}"),
    }
    match ProjectSection::resolve(
        "links",
        false,
        &[],
        &EmptyCatalog,
        is_reserved_drawer_key,
    ) {
        PlaceOutcome::Refuse { code, message } => {
            assert_eq!(code, "reserved_drawer");
            assert_eq!(
                message,
                "`LINKS` is managed by Bob; name the section something else"
            );
        }
        other => panic!("reserved project section: {other:?}"),
    }
}

#[test]
fn near_miss_thresholds_differ_by_scope() {
    let drawers = vec![drawer("GIT", 3)];
    match TaskDrawer::resolve(
        "gt",
        false,
        &drawers,
        &EmptyCatalog,
        is_reserved_drawer_key,
    ) {
        PlaceOutcome::NearMiss { code, message, .. } => {
            assert_eq!(code, Some("drawer_near_duplicate"));
            let message = message.expect("drawer near-miss copy");
            assert!(message.contains("#git"), "{message}");
        }
        other => panic!("drawer prefix/distance 1: {other:?}"),
    }
    match TaskDrawer::resolve(
        "gti",
        false,
        &drawers,
        &EmptyCatalog,
        is_reserved_drawer_key,
    ) {
        PlaceOutcome::Create { title, .. } => assert_eq!(title, "Gti"),
        other => panic!("drawer distance 2 creates: {other:?}"),
    }

    let headings = vec![heading("Git", 8)];
    match NotebookSection::resolve("gti", false, &headings) {
        PlaceOutcome::NearMiss {
            slug, quoted_title, ..
        } => {
            assert_eq!(slug, "git");
            assert_eq!(quoted_title, "Gti");
        }
        other => panic!("notebook distance 2: {other:?}"),
    }
    match NotebookSection::resolve("docker", false, &headings) {
        PlaceOutcome::Create { title, .. } => assert_eq!(title, "Docker"),
        other => panic!("unrelated slug creates: {other:?}"),
    }
}

#[test]
fn notebook_create_title_keeps_typed_capitals() {
    assert_eq!(format_notebook_section_create_title("API-keys"), "API Keys");
    assert_eq!(format_notebook_section_create_title("git"), "Git");
    assert_eq!(format_notebook_section_create_title("coc.nvim"), "Coc.nvim");
}

#[test]
fn catalog_explicit_spelling_beats_legacy_and_legacy_only_falls_back() {
    let catalog = MarkerCatalog {
        explicit: HashMap::from([(
            "FUTURE WORK".to_string(),
            "Future Work".to_string(),
        )]),
    };
    assert_eq!(
        create_title(TaskDrawer::resolve(
            "future-work",
            false,
            &[],
            &catalog,
            is_reserved_drawer_key,
        )),
        "Future Work"
    );
    assert_eq!(
        create_title(ProjectSection::resolve(
            "future-work",
            false,
            &[],
            &catalog,
            is_reserved_drawer_key,
        )),
        "Future Work"
    );
    assert_eq!(
        create_title(TaskDrawer::resolve(
            "api-keys",
            false,
            &[],
            &EmptyCatalog,
            is_reserved_drawer_key,
        )),
        "Api Keys"
    );
}

#[test]
fn quoted_create_preserves_punctuation_and_refuses_invalid_bold_headers() {
    for title in ["iOS / OAuth 2.0", "C++", "Q&A", "Café", "Q1: Goals"] {
        assert_eq!(
            create_title(TaskDrawer::resolve(
                title,
                true,
                &[],
                &EmptyCatalog,
                is_reserved_drawer_key,
            )),
            title
        );
    }
    match TaskDrawer::resolve(
        "NOTES:",
        true,
        &[],
        &EmptyCatalog,
        is_reserved_drawer_key,
    ) {
        PlaceOutcome::Refuse { code, message } => {
            assert_eq!(code, "unnameable_drawer");
            assert!(message.contains("NOTES:"), "{message}");
        }
        other => panic!("invalid bold create: {other:?}"),
    }
    let existing = vec![drawer("NOTES:", 4)];
    assert_eq!(
        existing_title(TaskDrawer::resolve(
            "NOTES:",
            true,
            &existing,
            &EmptyCatalog,
            is_reserved_drawer_key,
        )),
        "NOTES:"
    );
}
