use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;

use crate::native::{
    capture_pomodoros::{self, PomodoroState},
    capture_pomodoros_agenda,
    vault_links::NoteIndex,
};

use super::{
    attribute_session, fallback_hue, session_links, split_minutes,
    DiskHomeSource, HomeResolver, HomeSource, Hue, IndexedHomeSource,
};

#[derive(Debug, Deserialize)]
struct Vectors {
    notes: BTreeMap<PathBuf, String>,
    cases: Vec<VectorCase>,
}

#[derive(Debug, Deserialize)]
struct VectorCase {
    name: String,
    daily_path: PathBuf,
    exempt: Vec<String>,
    pomodoros: String,
    expected: Vec<Value>,
    #[serde(default)]
    expected_lead_homes: Vec<Option<String>>,
    #[serde(default)]
    expected_health: ExpectedHealth,
    #[serde(default)]
    rust_only: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ExpectedHealth {
    unfiled_links: u32,
    ambiguous_links: u32,
    unresolved_links: u32,
    rollup_invalid: usize,
    area_color_invalid: usize,
}

#[test]
fn shared_vectors_match_indexed_and_disk_sources() {
    let vectors: Vectors = serde_json::from_str(include_str!(
        "../../../tests/fixtures/pomodoro_homes/vectors.json"
    ))
    .expect("homes vectors parse");
    let note_index = NoteIndex::from_paths(vectors.notes.keys().cloned());
    let temp = tempfile::tempdir().expect("temporary mini-vault");
    for (path, contents) in &vectors.notes {
        let full_path = temp.path().join(path);
        fs::create_dir_all(full_path.parent().expect("note parent"))
            .expect("create mini-vault directory");
        fs::write(full_path, contents).expect("write mini-vault note");
    }

    for case in &vectors.cases {
        assert_eq!(
            case.rust_only,
            case.name == "ambiguous-basename",
            "only ambiguous-basename should be rust_only: {}",
            case.name,
        );
        let daily = format!("## Pomodoros\n{}", case.pomodoros);
        let indexed = evaluate_case(
            IndexedHomeSource::new(&note_index, &vectors.notes),
            case,
            &daily,
        );
        assert_eq!(indexed.0, case.expected, "indexed: {}", case.name);
        assert_eq!(
            indexed.2, case.expected_lead_homes,
            "indexed lead homes: {}",
            case.name
        );
        assert_health(&indexed.1, &case.expected_health, &case.name);

        let disk =
            evaluate_case(DiskHomeSource::new(temp.path()), case, &daily);
        assert_eq!(disk.0, case.expected, "disk: {}", case.name);
        assert_eq!(
            disk.2, case.expected_lead_homes,
            "disk lead homes: {}",
            case.name
        );
        assert_health(&disk.1, &case.expected_health, &case.name);
    }

    assert_eq!(fallback_hue("job"), Hue::Magenta);
}

#[test]
fn session_link_sibling_includes_struck_and_embedded_bare_links_only() {
    let daily = "## Pomodoros\n- [ ] ()\n  - [[plain#^a]]\n  - ~~[[struck#^b]]~~\n  - ![[embed#^c]]\n  - prose [[mixed#^d]]\n    - [[nested#^e]]\n  ```\n  [[fenced#^f]]\n  ```\n";
    let links = session_links(daily, 1);
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].target, "plain");
    assert_eq!(links[1].target, "struck");
    assert_eq!(links[2].target, "embed");
    assert!(links[2].embedded);

    let queued =
        crate::native::capture_pomodoro_start::list_queued_links(daily, 1);
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].path_part, "plain");
    assert_eq!(queued[1].path_part, "embed");
}

#[test]
fn split_minutes_uses_ledger_order_for_remainder() {
    assert_eq!(split_minutes(8, 3), vec![4, 2, 2]);
    assert_eq!(split_minutes(5, 3), vec![3, 1, 1]);
    assert_eq!(split_minutes(3, 0), Vec::<u32>::new());
}

#[test]
fn fallback_palette_fnv_vector_is_pinned() {
    assert_eq!(fallback_hue("job"), Hue::Magenta);
    assert_eq!(super::palette::fnv1a32(b"job"), 3_721_670_620);
}

#[test]
fn rollup_accepts_only_off_or_subprojects() {
    assert_eq!(super::rollup_value("off"), super::Rollup::Off);
    assert_eq!(
        super::rollup_value("SUBPROJECTS"),
        super::Rollup::Subprojects
    );
    assert_eq!(
        super::rollup_value("false"),
        super::Rollup::Invalid("false".to_string())
    );
}

#[test]
fn rolled_home_keeps_the_pre_rollup_note_chain() {
    let vectors: Vectors = serde_json::from_str(include_str!(
        "../../../tests/fixtures/pomodoro_homes/vectors.json"
    ))
    .expect("homes vectors parse");
    let note_index = NoteIndex::from_paths(vectors.notes.keys().cloned());
    let mut resolver =
        HomeResolver::new(IndexedHomeSource::new(&note_index, &vectors.notes));
    let home = resolver.resolve_note(Path::new("sase_blog_notes.md"));
    assert_eq!(home.key, "sase.md");
    assert_eq!(
        home.pre_rollup_chain,
        ["sase_blog_notes.md", "sase_blog.md", "sase.md", "dev.md"]
    );
}

fn evaluate_case<S: HomeSource>(
    source: S,
    case: &VectorCase,
    daily: &str,
) -> (Vec<Value>, super::ResolverHealth, Vec<Option<String>>) {
    let scan = capture_pomodoros::scan(daily);
    assert_eq!(
        scan.entries.len(),
        case.expected.len(),
        "entry count: {}",
        case.name,
    );
    let mut resolver = HomeResolver::new(source);
    let mut leads = Vec::new();
    let actual = scan
        .entries
        .iter()
        .map(|entry| {
            let links = session_links(daily, entry.line - 1);
            let minutes = if entry.state == PomodoroState::Completed {
                let raw_line = daily
                    .lines()
                    .nth(entry.line.saturating_sub(1))
                    .unwrap_or("");
                crate::native::capture::duration_from_range_text(raw_line)
                    .or_else(|| {
                        entry
                            .time_range
                            .as_deref()
                            .and_then(capture_pomodoros_agenda::range_minutes)
                    })
                    .and_then(|minutes| u32::try_from(minutes).ok())
            } else {
                None
            };
            let homes = attribute_session(
                entry.name.as_deref(),
                &links,
                entry.state == PomodoroState::Completed,
                minutes,
                &case.exempt,
                &case.daily_path,
                &mut resolver,
            );
            leads.push(homes.lead_home.as_ref().map(|home| home.key.clone()));
            serde_json::to_value(homes.homes).expect("homes JSON value")
        })
        .collect();
    (actual, resolver.health().clone(), leads)
}

fn assert_health(
    actual: &super::ResolverHealth,
    expected: &ExpectedHealth,
    case: &str,
) {
    assert_eq!(
        actual.unfiled_links, expected.unfiled_links,
        "unfiled: {case}"
    );
    assert_eq!(
        actual.ambiguous_links, expected.ambiguous_links,
        "ambiguous: {case}"
    );
    assert_eq!(
        actual.unresolved_links, expected.unresolved_links,
        "unresolved: {case}"
    );
    assert_eq!(
        actual.rollup_invalid.len(),
        expected.rollup_invalid,
        "rollup lints: {case}"
    );
    assert_eq!(
        actual.area_color_invalid.len(),
        expected.area_color_invalid,
        "area color lints: {case}"
    );
}
