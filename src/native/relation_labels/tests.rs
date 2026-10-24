//! Shared-vector coverage for the relation label vocabulary:
//! `tests/fixtures/relation_labels/vectors.json`.

use std::{collections::BTreeMap, fs};

use serde::{Deserialize, Serialize};

use crate::native::task_relations::label::inverse_phrase;

use super::{
    count_label,
    journal::{self, EventPayload},
    relation_label_key, sort_spellings, Catalog, HiddenFilter, LabelRecord,
};

#[derive(Debug, Deserialize)]
struct Vectors {
    key: Vec<KeyCase>,
    catalog: Vec<CatalogCase>,
    fold: Vec<FoldCase>,
    rank: Vec<RankCase>,
}

#[derive(Debug, Deserialize)]
struct KeyCase {
    name: String,
    input: String,
    expected: String,
}

#[derive(Debug, Deserialize)]
struct CatalogCase {
    name: String,
    #[serde(default)]
    warns: bool,
    notes: Vec<VectorNote>,
    expected: Vec<ExpectedRecord>,
}

#[derive(Debug, Deserialize)]
struct VectorNote {
    path: String,
    body: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ExpectedRecord {
    key: String,
    name: String,
    count: u64,
    spellings: Vec<ExpectedSpelling>,
    inverse: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ExpectedSpelling {
    name: String,
    count: u64,
}

#[derive(Debug, Deserialize)]
struct FoldCase {
    name: String,
    events: Vec<FoldEvent>,
    expected_hidden: Vec<String>,
    expected_names: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct FoldEvent {
    path: String,
    line: u32,
    body: String,
}

#[derive(Debug, Deserialize)]
struct RankCase {
    name: String,
    query: String,
    hidden: String,
    labels: Vec<RankFixture>,
    expected: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RankFixture {
    name: String,
    count: u64,
    hidden: bool,
}

fn load_vectors() -> Vectors {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/relation_labels/vectors.json"
    ))
    .expect("relation label vectors parse")
}

#[test]
fn shared_vectors_cover_key_catalog_fold_and_rank() {
    let vectors = load_vectors();
    assert!(!vectors.key.is_empty());
    assert!(!vectors.catalog.is_empty());
    assert!(!vectors.fold.is_empty());
    assert!(!vectors.rank.is_empty());
}

#[test]
fn vector_keys_match_the_one_key_function() {
    for case in &load_vectors().key {
        assert_eq!(
            relation_label_key(&case.input),
            case.expected,
            "key: {}",
            case.name
        );
    }
}

#[test]
fn vector_catalogs_scan_every_scope() {
    for case in &load_vectors().catalog {
        let temp = tempfile::tempdir().expect("scratch vault");
        for note in &case.notes {
            let path = temp.path().join(&note.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("note parent");
            }
            fs::write(&path, &note.body).expect("note body");
        }
        let catalog = Catalog::load(temp.path()).expect("catalog loads");
        let got: Vec<ExpectedRecord> = catalog
            .records()
            .map(|record| ExpectedRecord {
                key: record.key.clone(),
                name: record.name.clone(),
                count: record.count,
                spellings: record
                    .spellings
                    .iter()
                    .map(|spelling| ExpectedSpelling {
                        name: spelling.name.clone(),
                        count: spelling.count,
                    })
                    .collect(),
                inverse: record.inverse.clone(),
            })
            .collect();
        // Records iterate in key order; vectors list catalog order
        // (count desc, key, name). Re-sort the expectation the same way.
        let mut expected = case.expected.clone();
        expected.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.key.cmp(&right.key))
                .then_with(|| left.name.cmp(&right.name))
        });
        let mut got = got;
        got.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.key.cmp(&right.key))
                .then_with(|| left.name.cmp(&right.name))
        });
        assert_eq!(
            serde_json::to_value(&got).expect("got serializes"),
            serde_json::to_value(&expected).expect("expected serializes"),
            "catalog: {}",
            case.name
        );
        if case.warns {
            assert!(
                !catalog.warnings().is_empty(),
                "catalog: {} should warn",
                case.name
            );
        } else {
            assert!(
                catalog.warnings().is_empty(),
                "catalog: {} warnings: {:?}",
                case.name,
                catalog.warnings()
            );
        }
    }
}

#[test]
fn vector_folds_match_last_writer_wins() {
    for case in &load_vectors().fold {
        let mut parsed = Vec::new();
        for event in &case.events {
            // Malformed lines and foreign kinds never reach the fold.
            if let Some(event) =
                journal::parse_event(&event.body, &event.path, event.line)
            {
                parsed.push(event);
            }
        }
        let prefs = journal::fold_events(&parsed);
        let mut hidden: Vec<String> = prefs
            .names()
            .filter_map(|(key, _)| prefs.hidden(key).then(|| key.clone()))
            .collect();
        hidden.sort();
        let mut expected_hidden = case.expected_hidden.clone();
        expected_hidden.sort();
        assert_eq!(hidden, expected_hidden, "fold: {}", case.name);
        let names: BTreeMap<String, String> = prefs
            .names()
            .map(|(key, name)| (key.clone(), name.clone()))
            .collect();
        assert_eq!(names, case.expected_names, "fold: {}", case.name);
    }
}

#[test]
fn vector_ranks_match_prefix_then_substring() {
    for case in &load_vectors().rank {
        let mut rows: BTreeMap<String, LabelRecord> = BTreeMap::new();
        let mut seen = Vec::new();
        for fixture in &case.labels {
            for _ in 0..fixture.count {
                count_label(&mut rows, &mut seen, &fixture.name);
            }
        }
        for record in rows.values_mut() {
            sort_spellings(record);
        }
        let mut events = Vec::new();
        for fixture in case.labels.iter().filter(|label| label.hidden) {
            let key = relation_label_key(&fixture.name);
            events.push(journal::JournalEvent {
                ts: journal::event_timestamp(),
                host: "test".to_string(),
                path: "test.jsonl".to_string(),
                line: 1,
                payload: EventPayload::Hide {
                    key,
                    name: fixture.name.clone(),
                },
            });
        }
        let prefs = journal::fold_events(&events);
        let catalog = Catalog {
            records: rows,
            prefs,
            warnings: Vec::new(),
        };
        let filter = match case.hidden.as_str() {
            "only" => HiddenFilter::Only,
            "include" => HiddenFilter::Include,
            _ => HiddenFilter::Exclude,
        };
        let got: Vec<String> = catalog
            .rank_for_query(&case.query, filter)
            .iter()
            .map(|record| record.name.clone())
            .collect();
        assert_eq!(got, case.expected, "rank: {}", case.name);
    }
}

#[test]
fn journal_append_round_trips_through_the_host_file() {
    let temp = tempfile::tempdir().expect("scratch vault");
    let vault = temp.path();
    let ts = journal::event_timestamp();
    let line = journal::append_event_at(
        vault,
        "apollo",
        &journal::EventPayload::Hide {
            key: "see".to_string(),
            name: "see".to_string(),
        },
        "test",
        ts,
    )
    .expect("append");
    assert!(line.contains("\"kind\":\"relation_label.hide\""));
    let path = journal::host_journal_path(vault, "apollo");
    assert!(path.is_file(), "host file: {}", path.display());
    let (events, warnings) = journal::read_journal(vault).expect("read back");
    assert!(warnings.is_empty());
    assert_eq!(events.len(), 1);
    let prefs = journal::fold_events(&events);
    assert!(prefs.hidden("see"));
}

#[test]
fn inverse_phrases_match_the_contract_table() {
    for (stored, inverse) in [
        ("merged into", "merged from"),
        ("duplicate of", "duplicated by"),
        ("follow-up of", "followed up by"),
        ("superseded by", "supersedes"),
        ("split from", "split into"),
    ] {
        assert_eq!(inverse_phrase(stored), Some(inverse));
        assert_eq!(
            inverse_phrase(&stored.to_uppercase()),
            Some(inverse),
            "case-insensitive: {stored}"
        );
    }
    assert_eq!(inverse_phrase("see"), None);
    assert_eq!(inverse_phrase("followed up by"), None);
}
