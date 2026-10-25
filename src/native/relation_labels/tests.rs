//! Shared-vector coverage for the relation label vocabulary:
//! `tests/fixtures/relation_labels/vectors.json`.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

use crate::native::{env::TestEnvGuard, task_relations::label::inverse_phrase};

use super::{
    count_label,
    journal::{self, EventPayload},
    relation_label_key,
    scan::{self, CACHE_VERSION},
    sort_spellings, Catalog, HiddenFilter, LabelRecord,
};

fn isolate_catalog_env(cache: &std::path::Path) -> TestEnvGuard {
    TestEnvGuard::set(&[
        ("XDG_CACHE_HOME", Some(cache.as_os_str())),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent/bob-cli-test-config.yml")),
        ),
    ])
}

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
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
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

#[test]
fn catalog_scan_prunes_vanished_vaults_and_sweeps_orphan_files() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let live = tempfile::tempdir().expect("live vault");
    let vanished = tempfile::tempdir().expect("vanished vault");
    fs::write(
        live.path().join("tasks.md"),
        "- [ ] #task Live ^live\n  - 🪢 **LINKS:** see [[tasks#^live]]\n",
    )
    .expect("live note");

    // Prime the cache for the vault that is about to vanish, then
    // remove the vault directory itself.
    let vanished_cache = scan::cache_path(vanished.path());
    Catalog::load(vanished.path()).expect("prime vanished cache");
    assert!(vanished_cache.is_file(), "vanished cache is primed");

    let old_version = scan::relations_dir().join(format!(
        "catalog-v{}-{}.json",
        CACHE_VERSION - 1,
        "0".repeat(64)
    ));
    let unparseable = scan::relations_dir()
        .join(format!("catalog-v{CACHE_VERSION}-{}.json", "f".repeat(64)));
    let unrelated = scan::relations_dir().join("notes.txt");
    let stale_temp = scan::relations_dir().join(format!(
        ".catalog-v{CACHE_VERSION}-{}.json.123.tmp",
        "0".repeat(64)
    ));
    let fresh_temp = scan::relations_dir().join(format!(
        ".catalog-v{CACHE_VERSION}-{}.json.456.tmp",
        "0".repeat(64)
    ));
    let stale_old_temp = scan::relations_dir().join(format!(
        ".catalog-v{}-{}.json.789.tmp",
        CACHE_VERSION - 1,
        "0".repeat(64)
    ));
    let future_catalog = scan::relations_dir().join(format!(
        "catalog-v{}-{}.json",
        CACHE_VERSION + 1,
        "a".repeat(64)
    ));
    let future_temp = scan::relations_dir().join(format!(
        ".catalog-v{}-{}.json.999.tmp",
        CACHE_VERSION + 1,
        "a".repeat(64)
    ));
    let unrelated_tmp = scan::relations_dir().join("unrelated.tmp");
    let malformed_catalog = scan::relations_dir()
        .join(format!("catalog-v{CACHE_VERSION}-short.json"));
    let malformed_version_catalog = scan::relations_dir()
        .join(format!("catalog-vXYZ-{}.json", "0".repeat(64)));
    let malformed_temp_pid = scan::relations_dir().join(format!(
        ".catalog-v{CACHE_VERSION}-{}.json.abc.tmp",
        "0".repeat(64)
    ));
    let malformed_temp_hash = scan::relations_dir()
        .join(format!(".catalog-v{CACHE_VERSION}-short.json.123.tmp"));
    fs::write(&old_version, "stale version").unwrap();
    fs::write(&unparseable, "not json").unwrap();
    fs::write(&unrelated, "unrelated").unwrap();
    fs::write(&stale_temp, "stale write").unwrap();
    fs::write(&fresh_temp, "active write").unwrap();
    fs::write(&stale_old_temp, "stale older write").unwrap();
    fs::write(&future_catalog, "future version").unwrap();
    fs::write(&future_temp, "future write").unwrap();
    fs::write(&unrelated_tmp, "unrelated tmp").unwrap();
    fs::write(&malformed_catalog, "bad hash").unwrap();
    fs::write(&malformed_version_catalog, "bad version").unwrap();
    fs::write(&malformed_temp_pid, "bad pid").unwrap();
    fs::write(&malformed_temp_hash, "bad hash temp").unwrap();
    for aged in [
        &stale_temp,
        &stale_old_temp,
        &future_temp,
        &unrelated_tmp,
        &future_catalog,
        &malformed_catalog,
        &malformed_version_catalog,
        &malformed_temp_pid,
        &malformed_temp_hash,
    ] {
        fs::File::options()
            .write(true)
            .open(aged)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(
                SystemTime::now() - Duration::from_secs(10 * 60 + 1),
            ))
            .unwrap();
    }

    drop(vanished);

    let catalog = Catalog::load(live.path()).expect("live catalog loads");
    assert_eq!(catalog.known_count(), 1, "live scan finds its label");

    let live_cache = scan::cache_path(live.path());
    assert!(live_cache.is_file(), "live vault cache is kept");
    assert!(!vanished_cache.exists(), "vanished vault cache is removed");
    assert!(!old_version.exists(), "stale-version file is removed");
    assert!(!stale_temp.exists(), "stale atomic-write temp is removed");
    assert!(
        !stale_old_temp.exists(),
        "stale older-version atomic-write temp is removed"
    );
    assert!(fresh_temp.exists(), "fresh atomic-write temp is kept");
    assert!(
        unparseable.exists(),
        "unparseable current-version file is kept"
    );
    assert!(unrelated.exists(), "unrelated file is kept");
    assert!(unrelated_tmp.exists(), "aged unrelated tmp is preserved");
    assert!(
        future_catalog.exists(),
        "future-version catalog is preserved"
    );
    assert!(
        future_temp.exists(),
        "aged future-version atomic temp is preserved"
    );
    assert!(
        malformed_catalog.exists(),
        "malformed catalog name is preserved"
    );
    assert!(
        malformed_version_catalog.exists(),
        "non-numeric catalog version is preserved"
    );
    assert!(
        malformed_temp_pid.exists(),
        "non-numeric pid temp is preserved"
    );
    assert!(
        malformed_temp_hash.exists(),
        "malformed-hash temp is preserved"
    );
}
