use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::NaiveDate;
use serde::Deserialize;

use crate::native::env::TestEnvGuard;

use super::{
    classify::{self, is_machine_text},
    parse, rank, scan, LogKind, Phrase,
};

#[derive(Debug, Deserialize)]
struct Vectors {
    version: u32,
    source: String,
    normalize: Vec<NormalizeCase>,
    rank: Vec<RankCase>,
    machine_samples: Vec<Sample>,
    human_samples: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NormalizeCase {
    name: String,
    input: String,
    expected: String,
}

#[derive(Debug, Deserialize)]
struct RankCase {
    name: String,
    query: String,
    catalog: Vec<RankPhrase>,
    #[serde(default)]
    overlay: Vec<Overlay>,
    expected: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RankPhrase {
    text: String,
    key: String,
    score: f64,
    recency_rank: usize,
}

#[derive(Debug, Deserialize)]
struct Overlay {
    key: String,
    text: String,
    at: String,
}

#[derive(Debug, Deserialize)]
struct Sample {
    kind: String,
    source: String,
    text: String,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/log_recall/vectors.json"
    ))
    .expect("log recall vectors")
}

#[test]
fn shared_normalization_and_rank_vectors_match() {
    let vectors = vectors();
    assert_eq!(vectors.version, 1);
    assert_eq!(vectors.source, "tests/fixtures/log_recall/vectors.json");
    for case in vectors.normalize {
        assert_eq!(
            rank::normalize(&case.input),
            case.expected,
            "{}",
            case.name
        );
    }
    for case in vectors.rank {
        let phrases = apply_overlay(case.catalog, case.overlay);
        let actual = rank::rank(&phrases, Some(&case.query));
        assert_eq!(
            actual
                .iter()
                .map(|row| row.phrase.key.as_str())
                .collect::<Vec<_>>(),
            case.expected.iter().map(String::as_str).collect::<Vec<_>>(),
            "{}",
            case.name
        );
    }
}

fn apply_overlay(
    catalog: Vec<RankPhrase>,
    mut overlay: Vec<Overlay>,
) -> Vec<Phrase> {
    overlay.sort_by(|left, right| left.at.cmp(&right.at));
    let offset = overlay.len();
    let mut phrases = catalog
        .into_iter()
        .map(|row| Phrase {
            text: row.text,
            key: row.key,
            uses: 1,
            last_used: "2026-11-19".into(),
            score: row.score,
            recency_rank: row.recency_rank + offset,
        })
        .collect::<Vec<_>>();
    for (index, entry) in overlay.into_iter().enumerate() {
        let recency_rank = offset - index - 1;
        if let Some(existing) =
            phrases.iter_mut().find(|phrase| phrase.key == entry.key)
        {
            existing.text = entry.text;
            existing.recency_rank = recency_rank;
        } else {
            phrases.push(Phrase {
                text: entry.text,
                key: entry.key,
                uses: 1,
                last_used: "2026-11-19".into(),
                score: 1.0,
                recency_rank,
            });
        }
    }
    phrases
}

#[test]
fn machine_registry_samples_are_filtered_and_human_samples_survive() {
    let vectors = vectors();
    let registry = classify::machine_writers();
    for writer in &registry {
        assert!(
            is_machine_text(&writer.sample),
            "{}: {}",
            writer.helper,
            writer.sample
        );
    }
    let registered_samples = registry
        .iter()
        .map(|writer| writer.sample.as_str())
        .collect::<BTreeSet<_>>();
    let vector_samples = vectors
        .machine_samples
        .iter()
        .map(|sample| sample.text.as_str())
        .collect::<BTreeSet<_>>();
    for writer in &registry {
        assert!(
            vector_samples.contains(writer.sample.as_str()),
            "writer sample is missing from shared vectors: {} ({})",
            writer.sample,
            writer.source
        );
        let sample = vectors
            .machine_samples
            .iter()
            .find(|sample| sample.text == writer.sample)
            .expect("writer vector sample");
        assert_eq!(sample.kind, writer.kind.as_str(), "{}", writer.source);
    }
    for sample in &vectors.machine_samples {
        assert!(
            is_machine_text(&sample.text),
            "{} ({})",
            sample.text,
            sample.source
        );
        assert!(
            registered_samples.contains(sample.text.as_str()),
            "vector sample is absent from writer registry: {}",
            sample.text
        );
        assert!(LogKind::ALL.iter().any(|kind| kind.as_str() == sample.kind));
    }
    for sample in &vectors.human_samples {
        assert!(
            !is_machine_text(sample),
            "human vector classified as machine: {sample}"
        );
    }
}

#[test]
fn every_native_managed_log_writer_call_site_is_registered() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let native = root.join("src/native");
    let mut files = Vec::new();
    collect_rust_files(&native, &mut files);
    let expected = classify::writer_call_sites()
        .iter()
        .map(|(helper, path)| ((*helper).to_string(), (*path).to_string()))
        .collect::<BTreeSet<_>>();
    let registry = classify::machine_writers();
    for (helper, path) in &expected {
        assert!(
            registry
                .iter()
                .any(|writer| writer.helper == helper.as_str()
                    && writer.source == path),
            "writer call site {path}::{helper} has no registry sample"
        );
    }
    let mut found = BTreeSet::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative.starts_with("src/native/log_history/") {
            continue;
        }
        let source = fs::read_to_string(&file).expect("Rust source");
        let lines = source.lines().collect::<Vec<_>>();
        let test_module_start =
            lines.iter().enumerate().find_map(|(index, line)| {
                if line.trim() != "#[cfg(test)]" {
                    return None;
                }
                lines[index + 1..]
                    .iter()
                    .find(|next| !next.trim().is_empty())
                    .filter(|next| next.trim_start().starts_with("mod tests"))
                    .map(|_| index)
            });
        let production = &lines[..test_module_start.unwrap_or(lines.len())];
        for line in production {
            if line.contains("fn ") {
                continue;
            }
            for (helper, _) in &expected {
                let call = if helper == "cancel_reason" {
                    line.contains(".cancel_reason(")
                } else {
                    line.contains(&format!("{helper}("))
                };
                if call {
                    found.insert((helper.clone(), relative.clone()));
                }
            }
        }
    }
    assert_eq!(found, expected, "a managed-log writer call site changed; update the writer registry and samples");
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("source directory").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension() == Some(OsStr::new("rs")) {
            out.push(path);
        }
    }
}

#[test]
fn parses_direct_children_and_all_four_log_kinds() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
    let text = concat!(
        "- [ ] Work task ^work\r\n",
        "  - 🛠️ **WORK LOG**\r\n",
        "    - *2026-11-19 10:00* —   Athena   Agent  \r\n",
        "      - detail is not a phrase\r\n",
        "- [ ] Schedule task ^schedule\r\n",
        "  - 🗓️ **SCHEDULE LOG**\r\n",
        "    - *2026-11-22 → 2026-11-18* — Move it\r\n",
        "- [ ] Rotten task ^rotten\r\n",
        "  - 🥀 **ROTTEN LOG**\r\n",
        "    - *2026-11-18* — Keep · the middle · kept 3× in Next\r\n",
        "- [-] Cancel task ^cancel\r\n",
        "  - ❌ **CANCEL LOG**\r\n",
        "    - *2026-11-17* — Follow up\r\n",
        "- [ ] Unmanaged task ^unmanaged\r\n",
        "  - 🛠️ **WORK LOG IN PROGRESS**\r\n",
        "    - *2026-11-19 09:00* — Ignore me\r\n",
        "```md\r\n",
        "- [ ] Fenced task ^fenced\r\n",
        "  - 🛠️ **WORK LOG**\r\n",
        "    - *2026-11-19 08:00* — Ignore me\r\n",
        "```\r\n",
    );
    let scan = parse::scan_note(text, today);
    assert_eq!(scan.uses.len(), 4);
    assert!(scan.uses.iter().any(
        |entry| entry.kind == LogKind::Work && entry.key == "athena agent"
    ));
    assert!(scan.uses.iter().any(|entry| entry.kind == LogKind::Schedule
        && entry.last_used == "2026-11-18"));
    assert!(scan.uses.iter().any(|entry| entry.kind == LogKind::Rotten
        && entry.text == "Keep · the middle"));
    assert!(
        scan.uses
            .iter()
            .any(|entry| entry.kind == LogKind::Cancel
                && entry.key == "follow up")
    );
}

#[test]
fn excludes_machine_blank_overlong_and_unparsed_entries() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
    let long = "x".repeat(201);
    let text = format!(
        "- [ ] Task ^t\n  - 🛠️ **WORK LOG**\n    - *2026-11-19 10:00* — 🎲 automatic\n    - *2026-11-19 09:00* —   \n    - *2026-11-19 08:00* — {long}\n    - malformed log row\n"
    );
    let scan = parse::scan_note(&text, today);
    let counts = scan.excluded.get(&LogKind::Work).unwrap();
    assert_eq!(counts.machine, 1);
    assert_eq!(counts.blank, 1);
    assert_eq!(counts.too_long, 1);
    assert_eq!(counts.unparsed, 1);
}

#[test]
fn rotten_writer_tails_are_removed_only_at_the_end() {
    assert_eq!(parse::strip_rotten_writer_tail("A · kept 2×"), "A");
    assert_eq!(parse::strip_rotten_writer_tail("A · kept 2× at P1"), "A");
    assert_eq!(parse::strip_rotten_writer_tail("A · kept in Next"), "A");
    assert_eq!(
        parse::strip_rotten_writer_tail("A · stayed at P2 after 3 keeps"),
        "A"
    );
    assert_eq!(
        parse::strip_rotten_writer_tail("A · kept 2× then more"),
        "A · kept 2× then more"
    );
}

#[test]
fn log_scanner_deduplicates_copies_and_refreshes_without_a_ttl() {
    let vault = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::set(&[
        ("XDG_CACHE_HOME", Some(cache.path().as_os_str())),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent/log-recall-test.yml")),
        ),
    ]);
    fs::create_dir_all(vault.path().join("done")).unwrap();
    let body = "- [ ] Task ^t\n  - 🛠️ **WORK LOG**\n    - *2026-11-19 10:00* — Keep this\n";
    fs::write(vault.path().join("a.md"), body).unwrap();
    fs::write(vault.path().join("done/b.md"), body).unwrap();
    let first = scan::load(vault.path(), None).unwrap();
    assert_eq!(first.phrases_for(LogKind::Work)[0].uses, 1);
    assert_eq!(first.excluded_for(LogKind::Work).copies, 1);

    let changed = "- [ ] Task ^t\n  - 🛠️ **WORK LOG**\n    - *2026-11-19 11:00* — Fresh phrase\n";
    fs::write(vault.path().join("a.md"), changed).unwrap();
    fs::write(vault.path().join("done/b.md"), changed).unwrap();
    // This is the optional stale snapshot path used by keystroke callers.
    let within_ttl =
        scan::load(vault.path(), Some(Duration::from_secs(30))).unwrap();
    assert!(within_ttl
        .phrases_for(LogKind::Work)
        .iter()
        .any(|row| row.key == "keep this"));
    // The CLI path has no TTL and sees the edit immediately.
    let refreshed = scan::load(vault.path(), None).unwrap();
    assert!(refreshed
        .phrases_for(LogKind::Work)
        .iter()
        .any(|row| row.key == "fresh phrase"));
    assert!(!refreshed
        .phrases_for(LogKind::Work)
        .iter()
        .any(|row| row.key == "keep this"));

    let cache_dir = cache.path().join("bob-cli/log-history");
    let cache_file = fs::read_dir(&cache_dir)
        .unwrap()
        .flatten()
        .find(|entry| entry.path().extension() == Some(OsStr::new("json")))
        .unwrap()
        .path();
    fs::write(&cache_file, "corrupt").unwrap();
    let rebuilt = scan::load(vault.path(), None).unwrap();
    assert!(rebuilt
        .phrases_for(LogKind::Work)
        .iter()
        .any(|row| row.key == "fresh phrase"));
}

#[test]
fn shared_vectors_have_distinct_match_tiers() {
    let phrases = vec![
        phrase("exact", "ag", 0, 0.1),
        phrase("prefix", "agent", 3, 0.2),
        phrase("word", "athena agent", 0, 0.1),
        phrase("substring", "flagship", 2, 4.0),
    ];
    let ranked = rank::rank(&phrases, Some("ag"));
    assert_eq!(
        ranked.iter().map(|row| row.matched).collect::<Vec<_>>(),
        [
            Some(super::MatchKind::Exact),
            Some(super::MatchKind::Prefix),
            Some(super::MatchKind::Word),
            Some(super::MatchKind::Substring),
        ]
    );
}

fn phrase(text: &str, key: &str, recency_rank: usize, score: f64) -> Phrase {
    Phrase {
        text: text.into(),
        key: key.into(),
        uses: 1,
        last_used: "2026-11-19".into(),
        score,
        recency_rank,
    }
}
